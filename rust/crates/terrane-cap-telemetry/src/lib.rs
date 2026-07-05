//! The `telemetry` capability — structured app logging and error reporting.
//!
//! Most log chatter is deliberately transient: it lives in a host-side per-app
//! ring buffer (`$TERRANE_HOME/logs/<app>/current.jsonl`) and never enters the
//! event log, exactly like `sysinfo`'s live reads or `crypto`'s session keyring —
//! it is an observation of a run, not folded state. The line is drawn at crash
//! facts: an `error`-level call (or an auto-captured backend exception) records
//! exactly one `telemetry.error` event, so an admin console can fold error
//! counts and last-error facts and replay is intact.
//!
//! See `plan-completed-cap/cap-telemetry.md` for the locked decisions.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    truncate, CapManifest, Capability, CommandCtx, Decision, Effect,
    Error, EventPattern, EventRecord, EventSpec, GrantResourceSpec, ReadValue,
    RecordedCallCap, ResourceMethod, ResourceReadCtx, Result, StateStore,
};
pub use doc::telemetry_doc;

pub mod doc;

/// Maximum number of entries kept in the host-side per-app ring buffer. Rotated
/// jsonl files (`1.jsonl`…`N.jsonl`) carry the older tail; the host drops the
/// oldest when the per-app ceiling is reached. The buffer is host edge-only;
/// this constant only documents the contract for hosts and tests.
pub const RING_ROTATE_BYTES: u64 = 4 * 1024 * 1024;
pub const RING_ROTATE_KEEP: usize = 3;

/// Hard size cap on `msg` argument to any telemetry call. The decide path
/// truncates oversize input WITH a marker rather than erroring — a log call
/// should not crash the app — and the same canonical truncation runs when
/// folding a recorded `telemetry.error` event.
pub const MAX_MSG_BYTES: usize = 8 * 1024;
/// Hard size cap on `dataJson` argument.
pub const MAX_DATA_BYTES: usize = 32 * 1024;
/// Hard size cap on `stack` text carried by a `telemetry.error` event.
pub const MAX_STACK_BYTES: usize = 16 * 1024;
/// Maximum number of `ErrorFact`s kept in [`TelemetryState::last_errors`] per
/// app (the rest fall off the front); every error still bumps `error_count`.
pub const LAST_ERRORS_RING: usize = 20;
/// Same upper bound is exported as `per-app error *counter*` cap to the runner —
/// backstop against a runaway loop flooding the event log with errors.
pub const MAX_ERRORS_PER_RUN: usize = 1000;

/// Levels recognized by the `telemetry.*` resource surface. Lowercase ASCII
/// only, matching QuickJS' global `console` shim (`log`/`info`→`info`,
/// `warn`→`warn`, `error`→`error`, `debug`→`debug`).
pub const LEVELS: &[&str] = &["debug", "info", "warn", "error"];
/// `source` tags recorded on `telemetry.error` events.
pub const SOURCE_EXPLICIT: &str = "explicit";
pub const SOURCE_EXCEPTION: &str = "exception";
pub const SOURCE_TIMEOUT: &str = "timeout";
pub const SOURCE_FIRST_ERROR: &str = "first_error";

/// This capability's slice of State. Replay rebuilds it from
/// `telemetry.error` events alone; the jsonl ring buffer is non-authoritative
/// (like blob bytes minus the hash check) and never read in `fold`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryState {
    /// Total `telemetry.error` events folded for this app; monotonically
    /// non-decreasing while the app lives.
    pub error_count: BTreeMap<String, u64>,
    /// Ring of the last [`LAST_ERRORS_RING`] `ErrorFact`s per app, newest last.
    pub last_errors: BTreeMap<String, Vec<ErrorFact>>,
}

/// One folded error fact, kept in [`TelemetryState::last_errors`].
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ErrorFact {
    pub app: String,
    pub source: String,
    pub message: String,
    pub stack: String,
    pub data_digest: String,
}

/// Recorded payload of `telemetry.error`.
#[derive(BorshSerialize, BorshDeserialize)]
struct ErrorEvent {
    app: String,
    source: String,
    message: String,
    stack: String,
    data_digest: String,
}

/// Build a `telemetry.error` event from the owning edge path: an app-direct
/// `ctx.resource.telemetry.error(...)` call (`source = "explicit"`), or an
/// auto-captured backend exception / timeout / first-error
/// (`source = "exception" | "timeout" | "first_error"`). The `data` JSON is
/// left in the per-app jsonl buffer; only its sha256 (`data_digest`) enters the
/// recorded event so user data never ships in the syncable log.
pub fn error_event(
    app: &str,
    source: &str,
    message: &str,
    stack: &str,
    data: &str,
) -> Result<EventRecord> {
    if !is_valid_source(source) {
        return Err(Error::InvalidInput(format!(
            "invalid telemetry source: {source:?}"
        )));
    }
    encode_event(
        "telemetry.error",
        &ErrorEvent {
            app: app.to_string(),
            source: source.to_string(),
            message: truncate(message, MAX_MSG_BYTES),
            stack: truncate(stack, MAX_STACK_BYTES),
            data_digest: sha256_hex(data.as_bytes()),
        },
    )
}

/// Decode the digest out of a folded event; useful for assertion-only paths.
pub fn decode_error_event(record: &EventRecord) -> Result<ErrorFact> {
    let e: ErrorEvent = decode_event(record)?;
    Ok(ErrorFact {
        app: e.app,
        source: e.source,
        message: e.message,
        stack: e.stack,
        data_digest: e.data_digest,
    })
}

pub struct TelemetryCapability;

impl Capability for TelemetryCapability {
    fn namespace(&self) -> &'static str {
        "telemetry"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: vec![EventSpec {
                kind: "telemetry.error",
            }],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "debug",
                    params: &["msg", "dataJson?"],
                },
                ResourceMethod::Call {
                    name: "info",
                    params: &["msg", "dataJson?"],
                },
                ResourceMethod::Call {
                    name: "warn",
                    params: &["msg", "dataJson?"],
                },
                ResourceMethod::Call {
                    name: "error",
                    params: &["msg", "dataJson?"],
                },
                ResourceMethod::Read {
                    name: "read",
                    params: &["level?", "tail?"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "telemetry",
                &["call", "read"],
                "Structured app logging and reading back this app's own log buffer.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::telemetry_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        let method = name.split('.').nth(1).unwrap_or(name);
        match method {
            "debug" | "info" | "warn" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                let msg = truncate(&arg(args, 1, "msg")?, MAX_MSG_BYTES);
                let data = data_arg(args, 2)?;
                Ok(Decision::TransientEffect(Effect::AppLog {
                    app,
                    level: method.to_string(),
                    msg,
                    data,
                }))
            }
            "error" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                let msg = truncate(&arg(args, 1, "msg")?, MAX_MSG_BYTES);
                let data = data_arg(args, 2)?;
                Ok(Decision::Effect(Effect::AppLog {
                    app,
                    level: "error".to_string(),
                    msg,
                    data,
                }))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown command: telemetry.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "telemetry.error" => {
                let e: ErrorEvent = decode_event(record)?;
                let slice = state_mut::<TelemetryState>(state, "telemetry")?;
                let count = slice.error_count.entry(e.app.clone()).or_insert(0);
                *count += 1;
                let ring = slice.last_errors.entry(e.app.clone()).or_default();
                ring.push(ErrorFact {
                    app: e.app.clone(),
                    source: e.source,
                    message: e.message,
                    stack: e.stack,
                    data_digest: e.data_digest,
                });
                while ring.len() > LAST_ERRORS_RING {
                    ring.remove(0);
                }
            }
            "app.removed" => {
                let removed = decode_app_removed(record)?;
                let slice = state_mut::<TelemetryState>(state, "telemetry")?;
                slice.error_count.remove(&removed.id);
                slice.last_errors.remove(&removed.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "telemetry.error" {
            let e: ErrorEvent = decode_event(record).ok()?;
            Some(format!(
                "telemetry.error {} source={} (digest={})",
                e.app, e.source, e.data_digest
            ))
        } else {
            None
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        if name != "read" {
            return Err(Error::InvalidInput(format!(
                "unknown resource read: telemetry.{name}"
            )));
        }
        let app = ctx.app;
        ensure_app_exists(ctx.bus, app)?;
        let level = args.first().cloned().unwrap_or_default();
        let tail = args
            .get(1)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(200);
        let Some(host) = ctx.host else {
            return Err(Error::Runtime(
                "telemetry.read requires a live host with a log buffer".into(),
            ));
        };
        let json = host.sample(
            "telemetry.read",
            &[app.to_string(), level, tail.to_string()],
        )?;
        Ok(ReadValue::OptString(Some(json)))
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        _method: &str,
        _records: &[EventRecord],
    ) -> Result<ReadValue> {
        // Log calls return no value to JS — `console.log` prints and
        // `ctx.resource.telemetry.error(...)` returns `null`/`undefined`. A
        // buffer entry + (for error) the recorded event is the whole output.
        Ok(ReadValue::OptString(None))
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        match method {
            "error" => Some(RecordedCallCap {
                limit: MAX_ERRORS_PER_RUN,
                escape_hint:
                    "log at debug/info/warn (transient, buffers only, never recorded) instead of error if a loop is expected",
            }),
            _ => None,
        }
    }

    fn grant_resource_specs(&self) -> Vec<GrantResourceSpec> {
        self.manifest().grant_resources
    }
}

/// Accept `dataJson` (optional) truncated to [`MAX_DATA_BYTES`] with a marker.
/// Empty/missing resolves to the canonical empty-JSON string `"{}"` so the
/// edge always has a value to append.
pub fn data_arg(args: &[String], index: usize) -> Result<String> {
    let raw = args.get(index).cloned().unwrap_or_default();
    if raw.is_empty() {
        return Ok("{}".to_string());
    }
    Ok(truncate(&raw, MAX_DATA_BYTES))
}

pub fn is_valid_source(source: &str) -> bool {
    matches!(
        source,
        SOURCE_EXCEPTION | SOURCE_EXPLICIT | SOURCE_TIMEOUT | SOURCE_FIRST_ERROR
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Read-only access to the telemetry state slice for the cross-app catalog.
pub fn app_error_count(state: &dyn StateStore, app: &str) -> Option<u64> {
    state_ref::<TelemetryState>(state, "telemetry")
        .ok()
        .and_then(|s| s.error_count.get(app).copied())
}
