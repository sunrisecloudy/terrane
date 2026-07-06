//! The `interop` capability — recorded app-to-app backend calls.

use std::collections::{BTreeMap, VecDeque};

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use terrane_cap_app::AppState;
use terrane_cap_auth::namespace_granted;
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, ExecutionPrincipal, GrantResourceSpec, QueryCtx, QuerySpec,
    QueryValue, ReadValue, RecordedCallCap, ResourceMethod, Result, StateStore,
};

mod doc;

pub const MAX_ARGS_BYTES: usize = 64 * 1024;
pub const MAX_DEPTH: usize = 4;
pub const MAX_CALLS_PER_RUN: usize = 100;
pub const INLINE_REPLY_LIMIT: usize = 256 * 1024;
pub const BLOB_REPLY_LIMIT: usize = 8 * 1024 * 1024;
pub const PICKER_LIMIT: usize = 200;
pub const RECENT_PER_CALLER: usize = 64;

/// The common inbox verb `interop.send` delivers to on the picked target.
pub const RECEIVE_VERB: &str = "common.receive";

/// Marker token that fronts a picker-elicitation signal in an error message.
/// When a backend calls `interop.send`/`interop.pick` for an interface with no
/// recorded default target, the decide step fails with `<marker><json>` where
/// `<json>` is a [`PickRequired`] payload. Hosts (web shell + mac) detect this
/// token in the surfaced error, render the candidate app list, record the
/// user's choice as a grant, and retry — the powerbox flow. The token survives
/// the JS runtime because resource-call errors return the original `Error`
/// verbatim (see `run_js_bundle`).
pub const PICK_REQUIRED_MARKER: &str = "interop_pick_required:";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteropState {
    pub recent: BTreeMap<String, VecDeque<InteropCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteropCall {
    pub target: String,
    pub verb: String,
    pub args: Vec<String>,
    pub reply_kind: String,
    pub reply: String,
    pub reply_hash: String,
    pub ok: bool,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Called {
    caller: String,
    target: String,
    verb: String,
    args: Vec<String>,
    reply_kind: String,
    reply: String,
    reply_hash: String,
    ok: bool,
}

pub struct CalledEvent<'a> {
    pub caller: &'a str,
    pub target: &'a str,
    pub verb: &'a str,
    pub args: &'a [String],
    pub reply_kind: &'a str,
    pub reply: &'a str,
    pub reply_hash: &'a str,
    pub ok: bool,
}

pub fn called_event(input: CalledEvent<'_>) -> Result<EventRecord> {
    encode_event(
        "interop.called",
        &Called {
            caller: input.caller.to_string(),
            target: input.target.to_string(),
            verb: input.verb.to_string(),
            args: input.args.to_vec(),
            reply_kind: input.reply_kind.to_string(),
            reply: input.reply.to_string(),
            reply_hash: input.reply_hash.to_string(),
            ok: input.ok,
        },
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct InteropCapability;

impl Capability for InteropCapability {
    fn namespace(&self) -> &'static str {
        "interop"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "interop.call",
                },
                CommandSpec {
                    name: "interop.send",
                },
                CommandSpec {
                    name: "interop.pick",
                },
            ],
            events: vec![EventSpec {
                kind: "interop.called",
            }],
            queries: vec![QuerySpec {
                name: "interop.apps",
            }],
            resources: vec![
                ResourceMethod::Call {
                    name: "call",
                    params: &["target", "verb", "args"],
                },
                ResourceMethod::Call {
                    name: "send",
                    params: &["interface", "kind", "payloadJson"],
                },
                ResourceMethod::Call {
                    name: "pick",
                    params: &["interface"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "interop",
                &["call"],
                "Recorded app-to-app backend calls.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::interop_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "interop.call" => decide_call(ctx, args),
            "interop.send" => decide_send(ctx, args),
            "interop.pick" => decide_pick(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "apps" => {
                let interface = arg(args, 0, "interface")?;
                let json = apps_for_interface(ctx.state, &interface, PICKER_LIMIT)?;
                Ok(QueryValue::Json(json))
            }
            other => Err(Error::InvalidInput(format!("unknown query: interop.{other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "interop.called" => {
                let e: Called = decode_event(record)?;
                let recent = state_mut::<InteropState>(state, "interop")?
                    .recent
                    .entry(e.caller)
                    .or_default();
                recent.push_back(InteropCall {
                    target: e.target,
                    verb: e.verb,
                    args: e.args,
                    reply_kind: e.reply_kind,
                    reply: e.reply,
                    reply_hash: e.reply_hash,
                    ok: e.ok,
                });
                while recent.len() > RECENT_PER_CALLER {
                    recent.pop_front();
                }
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                let state = state_mut::<InteropState>(state, "interop")?;
                state.recent.remove(&e.id);
                for calls in state.recent.values_mut() {
                    calls.retain(|call| call.target != e.id);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind != "interop.called" {
            return None;
        }
        let e: Called = decode_event(record).ok()?;
        Some(format!(
            "interop.called {} -> {} {} ({})",
            e.caller, e.target, e.verb, e.reply_kind
        ))
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "call" | "send" | "pick" => {
                let record = records
                    .last()
                    .ok_or_else(|| Error::Runtime("interop call produced no result".into()))?;
                match record.kind.as_str() {
                    "interop.called" => {
                        let e: Called = decode_event(record)?;
                        if e.ok {
                            Ok(ReadValue::OptString(Some(e.reply)))
                        } else {
                            Err(Error::Runtime(e.reply))
                        }
                    }
                    "auth.granted" => Ok(ReadValue::OptString(Some("granted".to_string()))),
                    other => Err(Error::Runtime(format!(
                        "interop expected interop.called, got {other}"
                    ))),
                }
            }
            other => Err(Error::InvalidInput(format!(
                "interop.{other} is not a callable resource"
            ))),
        }
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        match method {
            "call" | "send" => Some(RecordedCallCap {
                limit: MAX_CALLS_PER_RUN,
                escape_hint: "batch interop work outside one backend run",
            }),
            _ => None,
        }
    }
}

fn decide_call(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let caller = arg(args, 0, "caller")?;
    let target = arg(args, 1, "target")?;
    let verb = arg(args, 2, "verb")?;
    let chain = parse_chain(args.get(3).map(String::as_str).unwrap_or(""), &caller)?;
    let call_args = args.get(4..).unwrap_or_default().to_vec();
    validate_call(ctx, &caller, &target, &verb, &chain, &call_args)?;
    Ok(Decision::Effect(Effect::AppCall {
        chain,
        target,
        verb,
        args: call_args,
    }))
}

fn decide_send(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let caller = arg(args, 0, "caller")?;
    let interface = arg(args, 1, "interface")?;
    let kind = arg(args, 2, "kind")?;
    // Payload is optional (empty for a bare ping); everything past it is ignored.
    let payload = args.get(3).cloned().unwrap_or_default();

    let principal = ExecutionPrincipal::local_owner();
    let Some(target) =
        terrane_cap_auth::interop_default_target(ctx.state, &principal, &caller, &interface)?
    else {
        // No default target yet: raise the picker. The host turns this marker
        // into a visual elicitation and, on the user's choice, records the
        // grant and retries the send.
        return Err(Error::InvalidInput(pick_required_marker(
            ctx.state, &caller, &interface,
        )?));
    };

    ensure_app_exists(ctx.bus, &caller)?;
    ensure_app_exists(ctx.bus, &target)?;
    if !app_declares_interface(ctx.state, &target, &interface)? {
        return Err(Error::InvalidInput(format!(
            "app {target} no longer declares interface {interface}; re-pick a target"
        )));
    }
    let call_args = vec![kind, payload];
    let total: usize = call_args.iter().map(|arg| arg.len()).sum();
    if total > MAX_ARGS_BYTES {
        return Err(Error::InvalidInput(format!(
            "interop args exceed {MAX_ARGS_BYTES} bytes"
        )));
    }
    Ok(Decision::Effect(Effect::AppCall {
        chain: vec![caller],
        target,
        verb: RECEIVE_VERB.to_string(),
        args: call_args,
    }))
}

fn decide_pick(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let caller = arg(args, 0, "caller")?;
    let interface = arg(args, 1, "interface")?;
    // A two-arg pick (from `ctx.resource.interop.pick(interface)`) has no target
    // yet — that is the user's choice. Raise the picker elicitation; the host
    // records the grant by re-dispatching this command with the chosen target.
    let Some(target) = args.get(2).filter(|t| !t.is_empty()).cloned() else {
        return Err(Error::InvalidInput(pick_required_marker(
            ctx.state, &caller, &interface,
        )?));
    };
    ensure_app_exists(ctx.bus, &caller)?;
    ensure_app_exists(ctx.bus, &target)?;
    if !app_declares_interface(ctx.state, &target, &interface)? {
        return Err(Error::InvalidInput(format!(
            "app {target} does not declare interface {interface}"
        )));
    }
    // Choosing IS granting: record the caller → interface → target default as a
    // scoped interop grant that `interop.send` resolves and `interop.call`
    // ignores (direct calls still need the blanket `interop` namespace grant).
    Ok(Decision::Commit(vec![
        terrane_cap_auth::granted_interop_target_event(
            &ExecutionPrincipal::local_owner(),
            &caller,
            &interface,
            &target,
        )?,
    ]))
}

/// Build the `interop_pick_required:<json>` signal an unresolved
/// `interop.send`/`interop.pick` fails with. The JSON carries the interface,
/// the requesting app, and the candidate apps declaring that interface so the
/// host can render the picker without a second round-trip.
fn pick_required_marker(state: &dyn StateStore, caller: &str, interface: &str) -> Result<String> {
    let apps = state_ref::<AppState>(state, "app")?;
    let candidates: Vec<serde_json::Value> = apps
        .apps
        .values()
        .filter(|app| app.interfaces.iter().any(|iface| iface == interface))
        .take(PICKER_LIMIT)
        .map(|app| serde_json::json!({ "id": app.id, "name": app.name }))
        .collect();
    let payload = serde_json::json!({
        "interface": interface,
        "app": caller,
        "candidates": candidates,
    });
    Ok(format!("{PICK_REQUIRED_MARKER}{payload}"))
}

fn validate_call(
    ctx: CommandCtx<'_>,
    caller: &str,
    target: &str,
    verb: &str,
    chain: &[String],
    args: &[String],
) -> Result<()> {
    ensure_app_exists(ctx.bus, caller)?;
    ensure_app_exists(ctx.bus, target)?;
    if verb.starts_with("__") {
        return Err(Error::InvalidInput(format!(
            "InteropInternalVerb: interop cannot call internal verb {verb}"
        )));
    }
    if chain.len() > MAX_DEPTH {
        return Err(Error::InvalidInput(format!(
            "InteropDepthExceeded: chain depth {} exceeds {MAX_DEPTH}",
            chain.len()
        )));
    }
    if chain.iter().any(|app| app == target) {
        return Err(Error::InvalidInput(format!(
            "InteropCycle: target {target} is already in call chain"
        )));
    }
    let total: usize = args.iter().map(|arg| arg.len()).sum();
    if total > MAX_ARGS_BYTES {
        return Err(Error::InvalidInput(format!(
            "interop args exceed {MAX_ARGS_BYTES} bytes"
        )));
    }
    let principal = ExecutionPrincipal::local_owner();
    if !namespace_granted(ctx.state, &principal, caller, "interop")? {
        return Err(Error::InvalidInput(format!(
            "permission required: grant interop to {caller}"
        )));
    }
    Ok(())
}

fn parse_chain(raw: &str, caller: &str) -> Result<Vec<String>> {
    let mut chain = if raw.trim().is_empty() {
        Vec::new()
    } else {
        raw.split('>').map(str::to_string).collect()
    };
    if chain.last().map(String::as_str) != Some(caller) {
        chain.push(caller.to_string());
    }
    Ok(chain)
}

fn apps_for_interface(state: &dyn StateStore, interface: &str, limit: usize) -> Result<String> {
    let apps = state_ref::<AppState>(state, "app")?;
    let mut values = Vec::new();
    for app in apps.apps.values() {
        if values.len() >= limit {
            break;
        }
        if app.interfaces.iter().any(|iface| iface == interface) {
            values.push(serde_json::json!({
                "id": app.id,
                "name": app.name,
                "interfaces": app.interfaces,
            }));
        }
    }
    Ok(serde_json::Value::Array(values).to_string())
}

fn app_declares_interface(state: &dyn StateStore, app: &str, interface: &str) -> Result<bool> {
    Ok(state_ref::<AppState>(state, "app")?
        .apps
        .get(app)
        .is_some_and(|record| record.interfaces.iter().any(|iface| iface == interface)))
}
