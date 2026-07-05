//! Inbound webhooks as recorded facts.
//!
//! The listener lives at the host edge. This capability owns route registration,
//! deterministic event shapes, header redaction, and replay state.

use std::collections::{BTreeMap, BTreeSet};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{json, Map, Value};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result,
    StateStore,
};
use terrane_cap_net::request::{is_sensitive_header, sha256_hex, REDACTED};

mod doc;

pub const MAX_HOOKS_PER_APP: usize = 32;
pub const MAX_NAME_LEN: usize = 128;
pub const MAX_HEADERS_BYTES: usize = 32 * 1024;
pub const INLINE_BODY_LIMIT: usize = 256 * 1024;
pub const BODY_HARD_LIMIT: usize = 32 * 1024 * 1024;
pub const RATE_LIMIT_PER_MINUTE: u32 = 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookState {
    pub routes: BTreeMap<AppId, BTreeMap<String, HookMeta>>,
    pub deliveries: BTreeMap<AppId, BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HookMeta {
    pub verb: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDelivery {
    pub app: String,
    pub name: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body_kind: String,
    pub body: String,
    pub body_is_base64: bool,
    pub body_hash: String,
    pub body_size: u64,
    pub body_mime: String,
    pub received_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Registered {
    app: String,
    name: String,
    verb: String,
    token: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Unregistered {
    app: String,
    name: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Received {
    app: String,
    name: String,
    method: String,
    headers: BTreeMap<String, String>,
    body_kind: String,
    body: String,
    body_is_base64: bool,
    body_hash: String,
    body_size: u64,
    body_mime: String,
    received_at: u64,
}

pub fn registered_event(
    app: impl Into<String>,
    name: impl Into<String>,
    verb: impl Into<String>,
    token: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "webhook.registered",
        &Registered {
            app: app.into(),
            name: name.into(),
            verb: verb.into(),
            token: token.into(),
        },
    )
}

pub fn rotated_event(
    app: impl Into<String>,
    name: impl Into<String>,
    verb: impl Into<String>,
    token: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "webhook.rotated",
        &Registered {
            app: app.into(),
            name: name.into(),
            verb: verb.into(),
            token: token.into(),
        },
    )
}

pub fn unregistered_event(app: impl Into<String>, name: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "webhook.unregistered",
        &Unregistered {
            app: app.into(),
            name: name.into(),
        },
    )
}

pub fn received_event(delivery: WebhookDelivery) -> Result<EventRecord> {
    encode_event(
        "webhook.received",
        &Received {
            app: delivery.app,
            name: delivery.name,
            method: delivery.method,
            headers: delivery.headers,
            body_kind: delivery.body_kind,
            body: delivery.body,
            body_is_base64: delivery.body_is_base64,
            body_hash: delivery.body_hash,
            body_size: delivery.body_size,
            body_mime: delivery.body_mime,
            received_at: delivery.received_at,
        },
    )
}

pub fn decode_delivery(record: &EventRecord) -> Result<WebhookDelivery> {
    let e: Received = decode_event(record)?;
    Ok(WebhookDelivery {
        app: e.app,
        name: e.name,
        method: e.method,
        headers: e.headers,
        body_kind: e.body_kind,
        body: e.body,
        body_is_base64: e.body_is_base64,
        body_hash: e.body_hash,
        body_size: e.body_size,
        body_mime: e.body_mime,
        received_at: e.received_at,
    })
}

pub fn route_matches(meta: &HookMeta, token: &str) -> bool {
    constant_time_eq(meta.token.as_bytes(), token.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    for i in 0..left.len().max(right.len()) {
        let a = left.get(i).copied().unwrap_or(0);
        let b = right.get(i).copied().unwrap_or(0);
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

pub struct WebhookCapability;

impl Capability for WebhookCapability {
    fn namespace(&self) -> &'static str {
        "webhook"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "webhook.register",
                },
                CommandSpec {
                    name: "webhook.rotate",
                },
                CommandSpec {
                    name: "webhook.unregister",
                },
                CommandSpec {
                    name: "webhook.ingest",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "webhook.registered",
                },
                EventSpec {
                    kind: "webhook.rotated",
                },
                EventSpec {
                    kind: "webhook.unregistered",
                },
                EventSpec {
                    kind: "webhook.received",
                },
            ],
            queries: Vec::new(),
            resources: vec![ResourceMethod::Read {
                name: "list",
                params: &[],
            }],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "webhook",
                &["read"],
                "Receive inbound HTTP from other software on your network.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::webhook_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "webhook.register" => decide_register(ctx, args),
            "webhook.rotate" => decide_rotate(ctx, args),
            "webhook.unregister" => decide_unregister(ctx, args),
            "webhook.ingest" => decide_ingest(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "webhook.registered" | "webhook.rotated" => {
                let e: Registered = decode_event(record)?;
                let state = state_mut::<WebhookState>(state, "webhook")?;
                state.routes.entry(e.app).or_default().insert(
                    e.name,
                    HookMeta {
                        verb: e.verb,
                        token: e.token,
                    },
                );
            }
            "webhook.unregistered" => {
                let e: Unregistered = decode_event(record)?;
                let state = state_mut::<WebhookState>(state, "webhook")?;
                if let Some(routes) = state.routes.get_mut(&e.app) {
                    routes.remove(&e.name);
                    if routes.is_empty() {
                        state.routes.remove(&e.app);
                    }
                }
                if let Some(deliveries) = state.deliveries.get_mut(&e.app) {
                    deliveries.remove(&e.name);
                    if deliveries.is_empty() {
                        state.deliveries.remove(&e.app);
                    }
                }
            }
            "webhook.received" => {
                let e: Received = decode_event(record)?;
                let state = state_mut::<WebhookState>(state, "webhook")?;
                *state
                    .deliveries
                    .entry(e.app)
                    .or_default()
                    .entry(e.name)
                    .or_default() += 1;
            }
            "app.removed" => {
                let removed = decode_app_removed(record)?;
                let state = state_mut::<WebhookState>(state, "webhook")?;
                state.routes.remove(&removed.id);
                state.deliveries.remove(&removed.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "webhook.registered" | "webhook.rotated" => {
                let e: Registered = decode_event(record).ok()?;
                Some(format!("{} {} -> {}", record.kind, e.name, e.verb))
            }
            "webhook.unregistered" => {
                let e: Unregistered = decode_event(record).ok()?;
                Some(format!("webhook.unregistered {}", e.name))
            }
            "webhook.received" => {
                let e: Received = decode_event(record).ok()?;
                Some(format!(
                    "webhook.received {} {} {} bytes",
                    e.name, e.method, e.body_size
                ))
            }
            _ => None,
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        _args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "list" => {
                let state = state_ref::<WebhookState>(ctx.state, "webhook")?;
                let hooks = state
                    .routes
                    .get(ctx.app)
                    .map(|routes| {
                        routes
                            .iter()
                            .map(|(name, meta)| {
                                json!({
                                    "name": name,
                                    "verb": meta.verb,
                                    "url_path": format!("/hook/{}/{}/{}", ctx.app, name, meta.token),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(ReadValue::OptString(Some(Value::Array(hooks).to_string())))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: webhook.{other}"
            ))),
        }
    }
}

fn decide_register(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    let verb = arg(args, 2, "verb")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_name(&name)?;
    validate_verb(&verb)?;
    let state = state_ref::<WebhookState>(ctx.state, "webhook")?;
    if !state
        .routes
        .get(&app)
        .map(|routes| routes.contains_key(&name))
        .unwrap_or(false)
        && state.routes.get(&app).map(BTreeMap::len).unwrap_or(0) >= MAX_HOOKS_PER_APP
    {
        return Err(Error::InvalidInput(format!(
            "webhook hooks per app must be <= {MAX_HOOKS_PER_APP}"
        )));
    }
    Ok(Decision::Effect(Effect::WebhookRegister { app, name, verb }))
}

fn decide_rotate(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_name(&name)?;
    let state = state_ref::<WebhookState>(ctx.state, "webhook")?;
    let meta = state
        .routes
        .get(&app)
        .and_then(|routes| routes.get(&name))
        .ok_or_else(|| Error::InvalidInput(format!("unknown webhook route: {app}/{name}")))?;
    Ok(Decision::Effect(Effect::WebhookRegister {
        app,
        name,
        verb: meta.verb.clone(),
    }))
}

fn decide_unregister(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_name(&name)?;
    Ok(Decision::Commit(vec![unregistered_event(app, name)?]))
}

fn decide_ingest(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let raw = arg(args, 0, "delivery_json")?;
    let delivery = prepare_delivery(ctx.state, &raw)?;
    Ok(Decision::Commit(vec![received_event(delivery)?]))
}

pub fn prepare_delivery(state: &dyn StateStore, raw: &str) -> Result<WebhookDelivery> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("webhook ingest must be JSON object: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("webhook ingest must be a JSON object".into()))?;
    let app = string_field(obj, "app")?;
    let name = string_field(obj, "name")?;
    let token = string_field(obj, "token")?;
    let method = string_field(obj, "method")?.to_ascii_uppercase();
    let received_at = u64_field(obj, "received_at")?;
    let mime = optional_string_field(obj, "body_mime")?.unwrap_or_else(|| "application/octet-stream".to_string());
    validate_name(&name)?;
    if method != "POST" {
        return Err(Error::InvalidInput("webhook.ingest only accepts POST deliveries".into()));
    }

    let state_ref = state_ref::<WebhookState>(state, "webhook")?;
    let meta = state_ref
        .routes
        .get(&app)
        .and_then(|routes| routes.get(&name))
        .ok_or_else(|| Error::InvalidInput("unknown webhook route".into()))?;
    if !route_matches(meta, &token) {
        return Err(Error::InvalidInput("unknown webhook route".into()));
    }

    let headers = redacted_headers(obj.get("headers"))?;
    let body = body_bytes(obj)?;
    if body.len() > BODY_HARD_LIMIT {
        return Err(Error::InvalidInput(format!(
            "webhook body must be <= {BODY_HARD_LIMIT} bytes"
        )));
    }
    let body_hash = sha256_hex(&body);
    let body_size = u64::try_from(body.len())
        .map_err(|_| Error::InvalidInput("webhook body length overflow".into()))?;
    let (body_kind, body_value, body_is_base64) = if body.len() <= INLINE_BODY_LIMIT {
        match std::str::from_utf8(&body) {
            Ok(text) => ("inline".to_string(), text.to_string(), false),
            Err(_) => ("inline".to_string(), B64.encode(&body), true),
        }
    } else {
        let blob_name = format!(
            "__webhook__/{}/{}/{}",
            app,
            name,
            state_ref
                .deliveries
                .get(&app)
                .and_then(|names| names.get(&name))
                .copied()
                .unwrap_or(0)
                + 1
        );
        ("blob".to_string(), blob_name, false)
    };

    Ok(WebhookDelivery {
        app,
        name,
        method,
        headers,
        body_kind,
        body: body_value,
        body_is_base64,
        body_hash,
        body_size,
        body_mime: mime,
        received_at,
    })
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > MAX_NAME_LEN
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "webhook name must be 1..={MAX_NAME_LEN} chars of [a-z0-9-_]"
        )));
    }
    Ok(())
}

fn validate_verb(verb: &str) -> Result<()> {
    if verb.trim().is_empty() {
        return Err(Error::InvalidInput("webhook verb must not be empty".into()));
    }
    Ok(())
}

fn string_field(obj: &Map<String, Value>, name: &str) -> Result<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
}

fn optional_string_field(obj: &Map<String, Value>, name: &str) -> Result<Option<String>> {
    obj.get(name)
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
        })
        .transpose()
}

fn u64_field(obj: &Map<String, Value>, name: &str) -> Result<u64> {
    obj.get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be an unsigned integer")))
}

fn redacted_headers(value: Option<&Value>) -> Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("headers must be a JSON object".into()))?;
    let mut total = 0usize;
    let mut out = BTreeMap::new();
    let app_declared = BTreeSet::new();
    for (name, value) in obj {
        let lower = name.to_ascii_lowercase();
        if lower.trim().is_empty() {
            return Err(Error::InvalidInput("header names must not be empty".into()));
        }
        let text = value
            .as_str()
            .ok_or_else(|| Error::InvalidInput("header values must be strings".into()))?;
        total = total
            .checked_add(lower.len())
            .and_then(|sum| sum.checked_add(text.len()))
            .ok_or_else(|| Error::InvalidInput("headers size overflow".into()))?;
        if total > MAX_HEADERS_BYTES {
            return Err(Error::InvalidInput(format!(
                "webhook headers must be <= {MAX_HEADERS_BYTES} bytes"
            )));
        }
        let record_value = if is_signature_header(&lower) {
            text.to_string()
        } else if is_sensitive_header(&lower, &app_declared) {
            REDACTED.to_string()
        } else {
            text.to_string()
        };
        out.insert(lower, record_value);
    }
    Ok(out)
}

fn is_signature_header(name: &str) -> bool {
    name.contains("signature") || name.ends_with("-signature-256")
}

fn body_bytes(obj: &Map<String, Value>) -> Result<Vec<u8>> {
    if let Some(body) = obj.get("body") {
        return body
            .as_str()
            .map(|s| s.as_bytes().to_vec())
            .ok_or_else(|| Error::InvalidInput("body must be a string".into()));
    }
    if let Some(raw) = obj.get("body_base64") {
        let raw = raw
            .as_str()
            .ok_or_else(|| Error::InvalidInput("body_base64 must be a string".into()))?;
        return B64
            .decode(raw)
            .map_err(|e| Error::InvalidInput(format!("body_base64 is invalid: {e}")));
    }
    Ok(Vec::new())
}
