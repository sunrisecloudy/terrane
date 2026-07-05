//! The `stream` capability — desired outbound SSE/WebSocket subscriptions and
//! recorded inbound messages.
//!
//! Opening a stream records desired state. The socket itself lives at the host
//! edge; every message observed there is recorded as a compact event and replay
//! folds those events without reopening sockets or rerunning backend JS.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result,
    StateStore,
};

mod doc;

pub const MAX_OPEN_STREAMS_PER_APP: usize = 16;
pub const MAX_NAME_LEN: usize = 128;
pub const INLINE_TEXT_LIMIT: usize = 256 * 1024;
pub const MAX_MESSAGE_SIZE: u64 = 8 * 1024 * 1024;
pub const RATE_LIMIT_PER_SECOND: u64 = 20;
pub const RATE_LIMIT_WINDOW_SECONDS: u64 = 10;
pub const REDACTED: &str = terrane_cap_net::request::REDACTED;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMeta {
    pub verb: String,
    pub kind: StreamKind,
    pub request_json_redacted: String,
    pub last_seq: u64,
    pub status: StreamStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Sse,
    Ws,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMessage {
    pub seq: u64,
    pub data_kind: String,
    pub data: String,
    pub data_is_base64: bool,
    pub data_hash: String,
    pub data_size: u64,
    pub received_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamState {
    pub streams: BTreeMap<AppId, BTreeMap<String, StreamMeta>>,
    pub messages: BTreeMap<AppId, BTreeMap<String, StreamMessage>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Opened {
    app: String,
    name: String,
    verb: String,
    kind: String,
    request_json_redacted: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Message {
    app: String,
    name: String,
    seq: u64,
    data_kind: String,
    data: String,
    data_is_base64: bool,
    data_hash: String,
    data_size: u64,
    received_at: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Reopened {
    app: String,
    name: String,
    seq_before: u64,
    attempt: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Closed {
    app: String,
    name: String,
    reason: String,
    by: String,
}

pub fn opened_event(
    app: impl Into<String>,
    name: impl Into<String>,
    verb: impl Into<String>,
    kind: StreamKind,
    request_json_redacted: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "stream.opened",
        &Opened {
            app: app.into(),
            name: name.into(),
            verb: verb.into(),
            kind: kind.as_str().to_string(),
            request_json_redacted: request_json_redacted.into(),
        },
    )
}

pub fn message_event(record: StreamMessageRecord) -> Result<EventRecord> {
    encode_event(
        "stream.message",
        &Message {
            app: record.app,
            name: record.name,
            seq: record.seq,
            data_kind: record.data_kind,
            data: record.data,
            data_is_base64: record.data_is_base64,
            data_hash: record.data_hash,
            data_size: record.data_size,
            received_at: record.received_at,
        },
    )
}

pub fn reopened_event(
    app: impl Into<String>,
    name: impl Into<String>,
    seq_before: u64,
    attempt: u64,
) -> Result<EventRecord> {
    encode_event(
        "stream.reopened",
        &Reopened {
            app: app.into(),
            name: name.into(),
            seq_before,
            attempt,
        },
    )
}

pub fn closed_event(
    app: impl Into<String>,
    name: impl Into<String>,
    reason: impl Into<String>,
    by: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "stream.closed",
        &Closed {
            app: app.into(),
            name: name.into(),
            reason: reason.into(),
            by: by.into(),
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMessageRecord {
    pub app: String,
    pub name: String,
    pub seq: u64,
    pub data_kind: String,
    pub data: String,
    pub data_is_base64: bool,
    pub data_hash: String,
    pub data_size: u64,
    pub received_at: String,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Sse => "sse",
            StreamKind::Ws => "ws",
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn next_seq(state: &StreamState, app: &str, name: &str) -> Result<u64> {
    let stream = state
        .streams
        .get(app)
        .and_then(|streams| streams.get(name))
        .ok_or_else(|| Error::InvalidInput(format!("unknown stream: {app}/{name}")))?;
    if stream.status != StreamStatus::Open {
        return Err(Error::InvalidInput(format!(
            "stream {app}/{name} is closed; cannot ingest"
        )));
    }
    Ok(stream.last_seq.saturating_add(1))
}

pub struct StreamCapability;

impl Capability for StreamCapability {
    fn namespace(&self) -> &'static str {
        "stream"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "stream.open",
                },
                CommandSpec {
                    name: "stream.close",
                },
                CommandSpec {
                    name: "stream.message",
                },
                CommandSpec {
                    name: "stream.reopened",
                },
                CommandSpec {
                    name: "stream.close-host",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "stream.opened",
                },
                EventSpec {
                    kind: "stream.message",
                },
                EventSpec {
                    kind: "stream.reopened",
                },
                EventSpec {
                    kind: "stream.closed",
                },
            ],
            queries: Vec::new(),
            resources: vec![ResourceMethod::Read {
                name: "list",
                params: &[],
            }],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "stream",
                &["read"],
                "Maintain live outbound WebSocket/SSE connections; every received message is recorded.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::stream_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "stream.open" => decide_open(ctx, args),
            "stream.close" => decide_close(ctx, args, "requested", "app"),
            "stream.message" => decide_message(ctx, args),
            "stream.reopened" => decide_reopened(ctx, args),
            "stream.close-host" => decide_close(ctx, args, arg(args, 2, "reason")?.as_str(), "host"),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        _args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "list" => Ok(ReadValue::OptString(Some(stream_list_json(ctx.state, ctx.app)?))),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: stream.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "stream.opened" => {
                let e: Opened = decode_event(record)?;
                let kind = parse_kind(&e.kind)?;
                let streams = state_mut::<StreamState>(state, "stream")?
                    .streams
                    .entry(e.app)
                    .or_default();
                streams.insert(
                    e.name,
                    StreamMeta {
                        verb: e.verb,
                        kind,
                        request_json_redacted: e.request_json_redacted,
                        last_seq: 0,
                        status: StreamStatus::Open,
                    },
                );
            }
            "stream.message" => {
                let e: Message = decode_event(record)?;
                let state = state_mut::<StreamState>(state, "stream")?;
                let stream = state
                    .streams
                    .get_mut(&e.app)
                    .and_then(|streams| streams.get_mut(&e.name))
                    .ok_or_else(|| Error::InvalidInput(format!(
                        "stream.message for unknown stream: {}/{}",
                        e.app, e.name
                    )))?;
                if e.seq <= stream.last_seq {
                    return Err(Error::InvalidInput(format!(
                        "stream.message seq regression for {}/{}: got {}, last {}",
                        e.app, e.name, e.seq, stream.last_seq
                    )));
                }
                stream.last_seq = e.seq;
                state
                    .messages
                    .entry(e.app)
                    .or_default()
                    .insert(
                        e.name,
                        StreamMessage {
                            seq: e.seq,
                            data_kind: e.data_kind,
                            data: e.data,
                            data_is_base64: e.data_is_base64,
                            data_hash: e.data_hash,
                            data_size: e.data_size,
                            received_at: e.received_at,
                        },
                    );
            }
            "stream.reopened" => {
                let e: Reopened = decode_event(record)?;
                if let Some(stream) = state_mut::<StreamState>(state, "stream")?
                    .streams
                    .get_mut(&e.app)
                    .and_then(|streams| streams.get_mut(&e.name))
                {
                    if e.seq_before > stream.last_seq {
                        stream.last_seq = e.seq_before;
                    }
                    stream.status = StreamStatus::Open;
                }
            }
            "stream.closed" => {
                let e: Closed = decode_event(record)?;
                if let Some(stream) = state_mut::<StreamState>(state, "stream")?
                    .streams
                    .get_mut(&e.app)
                    .and_then(|streams| streams.get_mut(&e.name))
                {
                    stream.status = StreamStatus::Closed;
                }
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                let state = state_mut::<StreamState>(state, "stream")?;
                state.streams.remove(&e.id);
                state.messages.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "stream.opened" => {
                let e: Opened = decode_event(record).ok()?;
                let host_path = serde_json::from_str::<Value>(&e.request_json_redacted)
                    .ok()
                    .and_then(|value| value.get("url").and_then(Value::as_str).map(host_path));
                Some(format!(
                    "stream.opened {}/{} {} {} {}",
                    e.app,
                    e.name,
                    e.kind,
                    e.verb,
                    host_path.unwrap_or_else(|| "<url>".to_string())
                ))
            }
            "stream.message" => {
                let e: Message = decode_event(record).ok()?;
                Some(format!(
                    "stream.message {}/{} #{} {} {} bytes {}",
                    e.app, e.name, e.seq, e.data_kind, e.data_size, e.received_at
                ))
            }
            "stream.reopened" => {
                let e: Reopened = decode_event(record).ok()?;
                Some(format!(
                    "stream.reopened {}/{} after #{} attempt {}",
                    e.app, e.name, e.seq_before, e.attempt
                ))
            }
            "stream.closed" => {
                let e: Closed = decode_event(record).ok()?;
                Some(format!(
                    "stream.closed {}/{} {} by {}",
                    e.app, e.name, e.reason, e.by
                ))
            }
            _ => None,
        }
    }
}

fn decide_open(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = validate_name(&arg(args, 1, "name")?)?;
    let verb = validate_verb(&arg(args, 2, "verb")?)?;
    let request_json = arg(args, 3, "request_json")?;
    ensure_app_exists(ctx.bus, &app)?;
    enforce_open_limit(ctx.state, &app, &name)?;
    let prepared = prepare_stream_request(&request_json)?;
    Ok(Decision::Commit(vec![opened_event(
        app,
        name,
        verb,
        prepared.kind,
        prepared.redacted_json,
    )?]))
}

fn decide_close(
    ctx: CommandCtx<'_>,
    args: &[String],
    reason: &str,
    by: &str,
) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = validate_name(&arg(args, 1, "name")?)?;
    ensure_stream_exists(ctx.state, &app, &name)?;
    validate_reason(reason)?;
    Ok(Decision::Commit(vec![closed_event(app, name, reason, by)?]))
}

fn decide_message(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = validate_name(&arg(args, 1, "name")?)?;
    let seq = parse_u64(&arg(args, 2, "seq")?, "seq")?;
    if seq == 0 {
        return Err(Error::InvalidInput("stream.message seq starts at 1".into()));
    }
    let data_kind = validate_data_kind(&arg(args, 3, "data_kind")?)?;
    let data = arg(args, 4, "data")?;
    let data_is_base64 = parse_bool(&arg(args, 5, "data_is_base64")?, "data_is_base64")?;
    let data_hash = validate_hash(&arg(args, 6, "data_hash")?)?;
    let data_size = parse_u64(&arg(args, 7, "data_size")?, "data_size")?;
    let received_at = non_empty(&arg(args, 8, "received_at")?, "received_at")?;
    if data_size > MAX_MESSAGE_SIZE {
        return Err(Error::InvalidInput(format!(
            "stream message exceeds {MAX_MESSAGE_SIZE} bytes"
        )));
    }
    let stream = state_ref::<StreamState>(ctx.state, "stream")?
        .streams
        .get(&app)
        .and_then(|streams| streams.get(&name))
        .ok_or_else(|| Error::InvalidInput(format!("unknown stream: {app}/{name}")))?;
    if stream.status != StreamStatus::Open {
        return Err(Error::InvalidInput(format!(
            "stream {app}/{name} is closed; cannot ingest"
        )));
    }
    if seq <= stream.last_seq {
        return Err(Error::InvalidInput(format!(
            "stream.message seq regression for {app}/{name}: got {seq}, last {}",
            stream.last_seq
        )));
    }
    if data_kind == "inline" && data_size > INLINE_TEXT_LIMIT as u64 {
        return Err(Error::InvalidInput(format!(
            "inline stream messages must be <= {INLINE_TEXT_LIMIT} bytes"
        )));
    }
    if data_kind == "blob" && !data.is_empty() && !data.starts_with("__stream__/") {
        return Err(Error::InvalidInput(
            "blob stream message data must be empty or a __stream__/ blob name".into(),
        ));
    }
    Ok(Decision::Commit(vec![message_event(StreamMessageRecord {
        app,
        name,
        seq,
        data_kind,
        data,
        data_is_base64,
        data_hash,
        data_size,
        received_at,
    })?]))
}

fn decide_reopened(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = validate_name(&arg(args, 1, "name")?)?;
    let seq_before = parse_u64(&arg(args, 2, "seq_before")?, "seq_before")?;
    let attempt = parse_u64(&arg(args, 3, "attempt")?, "attempt")?;
    ensure_stream_exists(ctx.state, &app, &name)?;
    if attempt == 0 {
        return Err(Error::InvalidInput("reopen attempt starts at 1".into()));
    }
    Ok(Decision::Commit(vec![reopened_event(
        app, name, seq_before, attempt,
    )?]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedStreamRequest {
    pub kind: StreamKind,
    pub redacted_json: String,
}

pub fn prepare_stream_request(raw: &str) -> Result<PreparedStreamRequest> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("stream request must be JSON object: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("stream request must be a JSON object".into()))?;
    let kind = parse_kind(
        obj.get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidInput("stream request missing kind".into()))?,
    )?;
    let url = obj
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("stream request missing url".into()))?;
    validate_stream_url(kind, url)?;

    if obj.contains_key("method") || obj.contains_key("body") {
        return Err(Error::InvalidInput(
            "stream request supports url, headers, sensitiveHeaders, timeoutMs, redirect, and kind; no method/body".into(),
        ));
    }
    let sensitive = sensitive_headers(obj.get("sensitiveHeaders"))?;
    let headers = headers_redacted(obj.get("headers"), &sensitive)?;
    let mut out = Map::new();
    out.insert("kind".to_string(), Value::String(kind.as_str().to_string()));
    out.insert("url".to_string(), Value::String(url.to_string()));
    out.insert("headers".to_string(), Value::Object(headers));
    if let Some(timeout) = obj.get("timeoutMs") {
        out.insert("timeoutMs".to_string(), timeout.clone());
    }
    if let Some(redirect) = obj.get("redirect") {
        out.insert("redirect".to_string(), redirect.clone());
    }
    let redacted_json = serde_json::to_string(&Value::Object(out))
        .map_err(|e| Error::InvalidInput(format!("redact stream request: {e}")))?;
    Ok(PreparedStreamRequest {
        kind,
        redacted_json,
    })
}

fn validate_stream_url(kind: StreamKind, url: &str) -> Result<()> {
    if url.trim().is_empty() {
        return Err(Error::InvalidInput("url must not be empty".into()));
    }
    let scheme = url.split_once("://").map(|(scheme, _)| scheme).unwrap_or("");
    let ok = match kind {
        StreamKind::Sse => matches!(scheme, "http" | "https"),
        StreamKind::Ws => matches!(scheme, "ws" | "wss"),
    };
    if !ok {
        return Err(Error::InvalidInput(format!(
            "stream kind {} does not support URL scheme {scheme:?}",
            kind.as_str()
        )));
    }
    if url.contains("169.254.169.254") {
        return Err(Error::InvalidInput(
            "stream request denied metadata service address".into(),
        ));
    }
    Ok(())
}

fn stream_list_json(state: &dyn StateStore, app: &str) -> Result<String> {
    let state = state_ref::<StreamState>(state, "stream")?;
    let mut rows = Vec::new();
    if let Some(streams) = state.streams.get(app) {
        for (name, stream) in streams {
            rows.push(serde_json::json!({
                "name": name,
                "kind": stream.kind.as_str(),
                "verb": stream.verb,
                "lastSeq": stream.last_seq,
                "status": if stream.status == StreamStatus::Open { "open" } else { "closed" },
            }));
        }
    }
    serde_json::to_string(&rows)
        .map_err(|e| Error::InvalidInput(format!("stream list encode failed: {e}")))
}

fn enforce_open_limit(state: &dyn StateStore, app: &str, name: &str) -> Result<()> {
    let state = state_ref::<StreamState>(state, "stream")?;
    let open = state
        .streams
        .get(app)
        .map(|streams| {
            streams
                .iter()
                .filter(|(existing, stream)| {
                    existing.as_str() != name && stream.status == StreamStatus::Open
                })
                .count()
        })
        .unwrap_or(0);
    if open >= MAX_OPEN_STREAMS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "stream app limit is {MAX_OPEN_STREAMS_PER_APP} open streams"
        )));
    }
    Ok(())
}

fn ensure_stream_exists(state: &dyn StateStore, app: &str, name: &str) -> Result<()> {
    if state_ref::<StreamState>(state, "stream")?
        .streams
        .get(app)
        .and_then(|streams| streams.get(name))
        .is_some()
    {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!("unknown stream: {app}/{name}")))
    }
}

fn validate_name(name: &str) -> Result<String> {
    if name.is_empty()
        || name.len() > MAX_NAME_LEN
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "stream name must be 1..={MAX_NAME_LEN} chars of [a-z0-9-_]"
        )));
    }
    Ok(name.to_string())
}

fn validate_verb(verb: &str) -> Result<String> {
    if verb.is_empty()
        || verb.len() > 128
        || !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(
            "stream verb must be a non-empty safe token".into(),
        ));
    }
    Ok(verb.to_string())
}

fn validate_reason(reason: &str) -> Result<()> {
    if reason.is_empty()
        || reason.len() > 128
        || !reason
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(
            "stream close reason must be a non-empty safe token".into(),
        ));
    }
    Ok(())
}

fn validate_data_kind(kind: &str) -> Result<String> {
    if matches!(kind, "inline" | "blob") {
        Ok(kind.to_string())
    } else {
        Err(Error::InvalidInput(format!(
            "stream data_kind must be inline or blob: {kind}"
        )))
    }
}

fn validate_hash(hash: &str) -> Result<String> {
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hash.to_ascii_lowercase())
    } else {
        Err(Error::InvalidInput("stream data_hash must be sha256 hex".into()))
    }
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be a non-negative integer: {raw}")))
}

fn parse_bool(raw: &str, label: &str) -> Result<bool> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::InvalidInput(format!("{label} must be true or false"))),
    }
}

fn parse_kind(raw: &str) -> Result<StreamKind> {
    match raw {
        "sse" => Ok(StreamKind::Sse),
        "ws" => Ok(StreamKind::Ws),
        other => Err(Error::InvalidInput(format!(
            "stream kind must be sse or ws: {other}"
        ))),
    }
}

fn non_empty(value: &str, label: &str) -> Result<String> {
    if value.trim().is_empty() {
        Err(Error::InvalidInput(format!("{label} must not be empty")))
    } else {
        Ok(value.to_string())
    }
}

fn sensitive_headers(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("sensitiveHeaders must be an array".into()))?;
    let mut out = Vec::new();
    for item in array {
        let name = item
            .as_str()
            .ok_or_else(|| Error::InvalidInput("sensitiveHeaders item must be a string".into()))?
            .to_ascii_lowercase();
        if name.trim().is_empty() {
            return Err(Error::InvalidInput(
                "sensitiveHeaders names must not be empty".into(),
            ));
        }
        out.push(name);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn headers_redacted(value: Option<&Value>, sensitive: &[String]) -> Result<Map<String, Value>> {
    let Some(value) = value else {
        return Ok(Map::new());
    };
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("headers must be a JSON object".into()))?;
    let mut pairs = BTreeMap::new();
    for (name, value) in obj {
        let name = name.to_ascii_lowercase();
        if name.trim().is_empty() {
            return Err(Error::InvalidInput("header names must not be empty".into()));
        }
        let value = if is_secret_marker(value)? {
            value.clone()
        } else if terrane_cap_net::request::is_sensitive_header(&name, &sensitive.iter().cloned().collect()) {
            Value::String(REDACTED.to_string())
        } else {
            value.as_str()
                .map(|s| Value::String(s.to_string()))
                .ok_or_else(|| {
                    Error::InvalidInput(
                        "header values must be strings or {\"$secret\":\"name\"}".into(),
                    )
                })?
        };
        pairs.insert(name, value);
    }
    Ok(pairs.into_iter().collect())
}

fn is_secret_marker(value: &Value) -> Result<bool> {
    let Some(obj) = value.as_object() else {
        return Ok(false);
    };
    if obj.len() == 1 {
        if let Some(secret) = obj.get("$secret") {
            let Some(secret) = secret.as_str() else {
                return Err(Error::InvalidInput("$secret must be a string".into()));
            };
            if secret.trim().is_empty() {
                return Err(Error::InvalidInput("$secret name must not be empty".into()));
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn host_path(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let without_fragment = after_scheme
        .split_once('#')
        .map(|(left, _)| left)
        .unwrap_or(after_scheme);
    let without_query = without_fragment
        .split_once('?')
        .map(|(left, _)| left)
        .unwrap_or(without_fragment);
    if without_query.is_empty() {
        "<url>".to_string()
    } else {
        without_query.to_string()
    }
}
