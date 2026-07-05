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

fn decide_send(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let caller = arg(args, 0, "caller")?;
    let interface = arg(args, 1, "interface")?;
    Err(Error::InvalidInput(format!(
        "interop.send for {caller}/{interface} needs a picker-selected default target; shell UI hook is not implemented yet"
    )))
}

fn decide_pick(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let caller = arg(args, 0, "caller")?;
    let interface = arg(args, 1, "interface")?;
    let target = arg(args, 2, "target")?;
    ensure_app_exists(ctx.bus, &caller)?;
    ensure_app_exists(ctx.bus, &target)?;
    if !app_declares_interface(ctx.state, &target, &interface)? {
        return Err(Error::InvalidInput(format!(
            "app {target} does not declare interface {interface}"
        )));
    }
    // The UI picker is a follow-up; this hook records the grant selected by the
    // caller/tool path today. Existing auth grants are namespace-shaped, so the
    // selector detail is encoded in the source string for audit visibility.
    Ok(Decision::Commit(vec![terrane_cap_auth::granted_namespace_event(
        &ExecutionPrincipal::local_owner(),
        &caller,
        "interop",
        &format!("interop:{interface}={target}"),
    )?]))
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
