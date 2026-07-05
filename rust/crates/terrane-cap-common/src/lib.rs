//! The `common` capability — outbound common messaging by channel.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error,
    EventPattern, EventRecord, EventSpec, ExecutionPrincipal, GrantResourceSpec, QueryCtx,
    QuerySpec, QueryValue, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
    LOCAL_OWNER_SUBJECT,
};

mod doc;
pub use doc::common_doc;

pub const CHANNEL_EMAIL: &str = "email";
pub const DEFAULT_EMAIL_CONNECTION: &str = "smtp-default";
pub const MAX_EMAIL_RECIPIENTS: usize = 20;
pub const MAX_EMAIL_SUBJECT_CHARS: usize = 998;
pub const MAX_EMAIL_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_EMAIL_HTML_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EMAIL_ATTACHMENTS: usize = 10;
pub const MAX_EMAIL_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_EMAIL_SENDS_PER_HOUR: usize = 20;
pub const MAX_EMAIL_SENDS_PER_DAY: usize = 100;
pub const RECORDED_BODY_INLINE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommonState {
    pub sent: BTreeMap<AppId, BTreeMap<String, SentMeta>>,
    pub attempts: BTreeMap<AppId, Vec<SendAttempt>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMeta {
    pub channel: String,
    pub to_count: usize,
    pub subject: Option<String>,
    pub body_hash: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAttempt {
    pub channel: String,
    pub sent_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SentAttachment {
    pub name: String,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Sent {
    app: String,
    channel: String,
    connection: String,
    message_id: String,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: Option<String>,
    body_hash: String,
    body_kind: String,
    body: String,
    attachments: Vec<SentAttachment>,
    status: String,
    error: String,
    sent_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSend {
    pub channel: String,
    pub connection: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: Option<String>,
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<PreparedAttachment>,
    pub record_body: bool,
    pub body_hash: String,
    pub body_kind: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_blob: Option<String>,
    pub sent_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAttachment {
    pub name: String,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

pub struct CommonCapability;

impl Capability for CommonCapability {
    fn namespace(&self) -> &'static str {
        "common"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "common.send",
            }],
            events: vec![EventSpec {
                kind: "common.sent",
            }],
            queries: vec![QuerySpec {
                name: "common.channels",
            }],
            resources: vec![
                ResourceMethod::Call {
                    name: "send",
                    params: &["messageJson"],
                },
                ResourceMethod::Read {
                    name: "status",
                    params: &["messageId"],
                },
                ResourceMethod::Read {
                    name: "channels",
                    params: &[],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "common",
                &["call", "read"],
                "Outbound common messaging resource table; each send is also gated by common:send:<channel>.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::common_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "common.send" => {
                let app = arg(args, 0, "app")?;
                let message_json = arg(args, 1, "message_json")?;
                ensure_app_exists(ctx.bus, &app)?;
                let prepared = prepare_send(ctx.state, &app, &message_json)?;
                ensure_channel_grant(ctx.state, &app, &prepared.channel)?;
                enforce_rate_limit(ctx.state, &app, &prepared.channel, prepared.sent_at)?;
                Ok(Decision::Effect(Effect::ChannelSend {
                    app,
                    channel: prepared.channel.clone(),
                    message: canonical_json(&prepared)?,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "common.sent" => {
                let e: Sent = decode_event(record)?;
                let attempt_at = parse_sent_at(&e.sent_at)?;
                let common = state_mut::<CommonState>(state, "common")?;
                common.attempts.entry(e.app.clone()).or_default().push(SendAttempt {
                    channel: e.channel.clone(),
                    sent_at: attempt_at,
                });
                common.sent.entry(e.app).or_default().insert(
                    e.message_id,
                    SentMeta {
                        channel: e.channel,
                        to_count: e.to.len() + e.cc.len() + e.bcc.len(),
                        subject: e.subject,
                        body_hash: e.body_hash,
                        status: e.status,
                    },
                );
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                let common = state_mut::<CommonState>(state, "common")?;
                common.sent.remove(&e.id);
                common.attempts.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind != "common.sent" {
            return None;
        }
        let e: Sent = decode_event(record).ok()?;
        let mut line = format!(
            "common.sent {} {} {} recipients subject={} status={}",
            e.app,
            e.channel,
            e.to.len() + e.cc.len() + e.bcc.len(),
            e.subject.unwrap_or_default(),
            e.status
        );
        if !e.error.is_empty() {
            line.push_str(" error=");
            line.push_str(&e.error);
        }
        Some(line)
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "channels" | "common.channels" => {
                let app = arg(args, 0, "app")?;
                Ok(QueryValue::Json(channels_json(ctx.state, &app)?))
            }
            other => Err(Error::InvalidInput(format!("unknown query: {other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "status" => {
                let message_id = arg(args, 0, "messageId")?;
                let meta = state_ref::<CommonState>(ctx.state, "common")?
                    .sent
                    .get(ctx.app)
                    .and_then(|messages| messages.get(&message_id));
                Ok(ReadValue::OptString(meta.map(status_json)))
            }
            "channels" => Ok(ReadValue::OptString(Some(channels_json(ctx.state, ctx.app)?))),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: common.{other}"
            ))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "send" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "common.sent")
                    .ok_or_else(|| Error::Runtime("common.send produced no send event".into()))?;
                let e: Sent = decode_event(record)?;
                Ok(ReadValue::OptString(Some(serde_json::json!({
                    "message_id": e.message_id,
                    "status": e.status,
                    "error": if e.error.is_empty() { Value::Null } else { Value::String(e.error) },
                }).to_string())))
            }
            other => Err(Error::InvalidInput(format!(
                "common.{other} is not a callable resource"
            ))),
        }
    }
}

pub fn prepare_send(state: &dyn StateStore, app: &str, raw: &str) -> Result<PreparedSend> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("common.send message_json must be JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("common.send message_json must be an object".into()))?;
    let channel = string_field(obj, "channel")?;
    if channel != CHANNEL_EMAIL {
        return Err(Error::InvalidInput(format!(
            "unknown common.send channel {channel:?}; configured channels: email"
        )));
    }
    let to = recipients(obj, "to")?;
    let cc = optional_recipients(obj, "cc")?;
    let bcc = optional_recipients(obj, "bcc")?;
    let total = to.len() + cc.len() + bcc.len();
    if total == 0 || total > MAX_EMAIL_RECIPIENTS {
        return Err(Error::InvalidInput(format!(
            "email recipients must total 1..={MAX_EMAIL_RECIPIENTS}"
        )));
    }
    for addr in to.iter().chain(cc.iter()).chain(bcc.iter()) {
        validate_email_addr(addr)?;
    }
    let subject = optional_string(obj, "subject")?;
    if subject
        .as_ref()
        .map(|value| value.chars().count() > MAX_EMAIL_SUBJECT_CHARS)
        .unwrap_or(false)
    {
        return Err(Error::InvalidInput(format!(
            "email subject exceeds {MAX_EMAIL_SUBJECT_CHARS} chars"
        )));
    }
    let text = string_field(obj, "text")?;
    if text.is_empty() {
        return Err(Error::InvalidInput("email text body must not be empty".into()));
    }
    if text.len() > MAX_EMAIL_TEXT_BYTES {
        return Err(Error::InvalidInput(format!(
            "email text body exceeds {MAX_EMAIL_TEXT_BYTES} bytes"
        )));
    }
    let html = optional_string(obj, "html")?;
    if html
        .as_ref()
        .map(|value| value.len() > MAX_EMAIL_HTML_BYTES)
        .unwrap_or(false)
    {
        return Err(Error::InvalidInput(format!(
            "email html body exceeds {MAX_EMAIL_HTML_BYTES} bytes"
        )));
    }
    let attachments = attachments(state, app, obj.get("attachments"))?;
    let record_body = optional_bool(obj, "recordBody")?.unwrap_or(false);
    let connection = optional_string(obj, "connection")?
        .unwrap_or_else(|| DEFAULT_EMAIL_CONNECTION.to_string());
    terrane_cap_connection::validate_name(&connection)?;
    let body_hash = sha256_hex(body_bytes(&text, html.as_deref()).as_slice());
    let (body_kind, body, body_blob) = recorded_body(&text, html.as_deref(), record_body)?;
    let sent_at = optional_u64(obj, "sentAt")?;
    Ok(PreparedSend {
        channel,
        connection,
        to,
        cc,
        bcc,
        subject,
        text,
        html,
        attachments,
        record_body,
        body_hash,
        body_kind,
        body,
        body_blob,
        sent_at,
    })
}

pub fn sent_event(
    app: &str,
    prepared: &PreparedSend,
    message_id: &str,
    status: &str,
    error: &str,
    sent_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "common.sent",
        &Sent {
            app: app.to_string(),
            channel: prepared.channel.clone(),
            connection: prepared.connection.clone(),
            message_id: message_id.to_string(),
            to: prepared.to.clone(),
            cc: prepared.cc.clone(),
            bcc: prepared.bcc.clone(),
            subject: prepared.subject.clone(),
            body_hash: prepared.body_hash.clone(),
            body_kind: prepared.body_kind.clone(),
            body: prepared.body.clone(),
            attachments: prepared
                .attachments
                .iter()
                .map(|a| SentAttachment {
                    name: a.name.clone(),
                    hash: a.hash.clone(),
                    size: a.size,
                    mime: a.mime.clone(),
                })
                .collect(),
            status: status.to_string(),
            error: error.to_string(),
            sent_at: sent_at.to_string(),
        },
    )
}

pub fn decode_prepared_send(raw: &str) -> Result<PreparedSend> {
    serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("common.send prepared message must be JSON: {e}")))
}

pub fn channel_resource_id(channel: &str) -> Result<String> {
    if channel == CHANNEL_EMAIL {
        Ok("common:send:email".to_string())
    } else {
        Err(Error::InvalidInput(format!(
            "unknown common.send channel {channel:?}; configured channels: email"
        )))
    }
}

fn ensure_channel_grant(state: &dyn StateStore, app: &str, channel: &str) -> Result<()> {
    let resource_id = channel_resource_id(channel)?;
    let principal = ExecutionPrincipal::local_owner();
    if terrane_cap_auth::resource_granted(state, &principal, app, &resource_id)? {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "permission required: grant {resource_id} to {app} for {LOCAL_OWNER_SUBJECT}"
    )))
}

fn enforce_rate_limit(
    state: &dyn StateStore,
    app: &str,
    channel: &str,
    candidate_sent_at: Option<u64>,
) -> Result<()> {
    let attempts = state_ref::<CommonState>(state, "common")?
        .attempts
        .get(app)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let now = candidate_sent_at
        .or_else(|| attempts.iter().map(|attempt| attempt.sent_at).max().map(|v| v + 1))
        .unwrap_or(0);
    let hour_start = now.saturating_sub(60 * 60);
    let day_start = now.saturating_sub(24 * 60 * 60);
    let hour = attempts
        .iter()
        .filter(|attempt| attempt.channel == channel && attempt.sent_at >= hour_start)
        .count();
    if hour >= MAX_EMAIL_SENDS_PER_HOUR {
        return Err(Error::InvalidInput(format!(
            "common.send email rate limit exceeded: {MAX_EMAIL_SENDS_PER_HOUR}/hour"
        )));
    }
    let day = attempts
        .iter()
        .filter(|attempt| attempt.channel == channel && attempt.sent_at >= day_start)
        .count();
    if day >= MAX_EMAIL_SENDS_PER_DAY {
        return Err(Error::InvalidInput(format!(
            "common.send email rate limit exceeded: {MAX_EMAIL_SENDS_PER_DAY}/day"
        )));
    }
    Ok(())
}

fn channels_json(state: &dyn StateStore, app: &str) -> Result<String> {
    let configured = terrane_cap_connection::status(state, DEFAULT_EMAIL_CONNECTION)?
        .map(|status| status.kind == "smtp")
        .unwrap_or(false);
    let granted = terrane_cap_auth::resource_granted(
        state,
        &ExecutionPrincipal::local_owner(),
        app,
        "common:send:email",
    )?;
    Ok(serde_json::json!({
        "email": {
            "configured": configured,
            "granted": granted,
            "resource": "common:send:email",
            "connection": DEFAULT_EMAIL_CONNECTION,
        }
    })
    .to_string())
}

fn status_json(meta: &SentMeta) -> String {
    serde_json::json!({
        "channel": meta.channel,
        "to_count": meta.to_count,
        "subject": meta.subject,
        "body_hash": meta.body_hash,
        "status": meta.status,
    })
    .to_string()
}

fn canonical_json(prepared: &PreparedSend) -> Result<String> {
    serde_json::to_string(prepared)
        .map_err(|e| Error::InvalidInput(format!("canonicalize common.send: {e}")))
}

fn string_field(obj: &serde_json::Map<String, Value>, name: &str) -> Result<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("email {name} must be a string")))
}

fn optional_string(obj: &serde_json::Map<String, Value>, name: &str) -> Result<Option<String>> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| Error::InvalidInput(format!("email {name} must be a string"))),
    }
}

fn optional_bool(obj: &serde_json::Map<String, Value>, name: &str) -> Result<Option<bool>> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| Error::InvalidInput(format!("email {name} must be a boolean"))),
    }
}

fn optional_u64(obj: &serde_json::Map<String, Value>, name: &str) -> Result<Option<u64>> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| Error::InvalidInput(format!("email {name} must be a u64"))),
    }
}

fn recipients(obj: &serde_json::Map<String, Value>, name: &str) -> Result<Vec<String>> {
    let value = obj
        .get(name)
        .ok_or_else(|| Error::InvalidInput(format!("email {name} is required")))?;
    recipient_array(value, name)
}

fn optional_recipients(obj: &serde_json::Map<String, Value>, name: &str) -> Result<Vec<String>> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => recipient_array(value, name),
    }
}

fn recipient_array(value: &Value, name: &str) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput(format!("email {name} must be an array")))?;
    items
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| Error::InvalidInput(format!("email {name} entries must be strings")))
        })
        .collect()
}

fn validate_email_addr(addr: &str) -> Result<()> {
    let Some((local, domain)) = addr.split_once('@') else {
        return Err(Error::InvalidInput(format!("invalid email recipient: {addr}")));
    };
    if local.is_empty()
        || domain.is_empty()
        || local.len() > 64
        || domain.len() > 255
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
        || addr.chars().any(char::is_whitespace)
    {
        return Err(Error::InvalidInput(format!("invalid email recipient: {addr}")));
    }
    Ok(())
}

fn attachments(
    state: &dyn StateStore,
    app: &str,
    value: Option<&Value>,
) -> Result<Vec<PreparedAttachment>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("email attachments must be an array".into()))?;
    if items.len() > MAX_EMAIL_ATTACHMENTS {
        return Err(Error::InvalidInput(format!(
            "email attachments exceed {MAX_EMAIL_ATTACHMENTS}"
        )));
    }
    let blob_state = state_ref::<terrane_cap_blob::BlobState>(state, "blob")?;
    let names = blob_state.blobs.get(app);
    let mut out = Vec::with_capacity(items.len());
    let mut total = 0u64;
    for value in items {
        let name = value
            .as_str()
            .ok_or_else(|| Error::InvalidInput("email attachment names must be strings".into()))?;
        let meta = names
            .and_then(|names| names.get(name))
            .ok_or_else(|| Error::KeyNotFound(app.to_string(), name.to_string()))?;
        total = total.saturating_add(meta.size);
        if total > MAX_EMAIL_ATTACHMENT_BYTES {
            return Err(Error::InvalidInput(format!(
                "email attachments exceed {MAX_EMAIL_ATTACHMENT_BYTES} bytes total"
            )));
        }
        out.push(PreparedAttachment {
            name: name.to_string(),
            hash: meta.hash.clone(),
            size: meta.size,
            mime: meta.mime.clone(),
        });
    }
    Ok(out)
}

fn body_bytes(text: &str, html: Option<&str>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + html.map(str::len).unwrap_or(0) + 1);
    bytes.extend_from_slice(text.as_bytes());
    if let Some(html) = html {
        bytes.push(0);
        bytes.extend_from_slice(html.as_bytes());
    }
    bytes
}

fn recorded_body(
    text: &str,
    html: Option<&str>,
    record_body: bool,
) -> Result<(String, String, Option<String>)> {
    if !record_body {
        return Ok(("none".to_string(), String::new(), None));
    }
    let body = match html {
        Some(html) => serde_json::json!({ "text": text, "html": html }).to_string(),
        None => text.to_string(),
    };
    if body.len() <= RECORDED_BODY_INLINE_LIMIT {
        return Ok(("inline".to_string(), body, None));
    }
    let hash = sha256_hex(body.as_bytes());
    Ok(("blob".to_string(), hash, Some(body)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn parse_sent_at(raw: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::Storage(format!("corrupt common.sent sent_at: {raw}")))
}
