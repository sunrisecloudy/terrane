//! The `mcp` capability — recorded app calls to external MCP servers.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error,
    EventPattern, EventRecord, EventSpec, ExecutionPrincipal, GrantResourceSpec, ReadValue,
    RecordedCallCap, ResourceMethod, Result, StateStore, LOCAL_OWNER_SUBJECT,
};

mod doc;

pub const MAX_CONNECTIONS: usize = 16;
pub const MAX_ARGS_BYTES: usize = 128 * 1024;
pub const INLINE_RESULT_LIMIT: usize = 256 * 1024;
pub const RESULT_HARD_LIMIT: usize = 32 * 1024 * 1024;
pub const MAX_CALLS_PER_RUN: usize = 60;
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const MAX_TIMEOUT_MS: u64 = 300_000;
pub const REDACTED: &str = "«redacted»";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpClientState {
    pub connections: BTreeMap<String, String>,
    pub calls: BTreeMap<AppId, BTreeMap<String, RecordedCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub connection: String,
    pub tool: String,
    pub args_json_redacted: String,
    pub result_kind: String,
    pub result: String,
    pub result_is_base64: bool,
    pub result_hash: String,
    pub result_size: u64,
    pub is_error: bool,
    pub called_at: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Connected {
    name: String,
    transport_json_redacted: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Disconnected {
    name: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Called {
    app: String,
    connection: String,
    tool: String,
    args_json_redacted: String,
    result_kind: String,
    result: String,
    result_is_base64: bool,
    result_hash: String,
    result_size: u64,
    is_error: bool,
    called_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCall {
    pub connection: String,
    pub tool: String,
    pub args_json: String,
    pub args_json_redacted: String,
    pub call_key: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalledEvent {
    pub app: String,
    pub connection: String,
    pub tool: String,
    pub args_json_redacted: String,
    pub result_kind: String,
    pub result: String,
    pub result_is_base64: bool,
    pub result_hash: String,
    pub result_size: u64,
    pub is_error: bool,
    pub called_at: String,
}

pub fn connected_event(name: &str, transport_json_redacted: &str) -> Result<EventRecord> {
    encode_event(
        "mcp.connected",
        &Connected {
            name: name.to_string(),
            transport_json_redacted: transport_json_redacted.to_string(),
        },
    )
}

pub fn disconnected_event(name: &str) -> Result<EventRecord> {
    encode_event(
        "mcp.disconnected",
        &Disconnected {
            name: name.to_string(),
        },
    )
}

pub fn called_event(input: CalledEvent) -> Result<EventRecord> {
    encode_event(
        "mcp.called",
        &Called {
            app: input.app,
            connection: input.connection,
            tool: input.tool,
            args_json_redacted: input.args_json_redacted,
            result_kind: input.result_kind,
            result: input.result,
            result_is_base64: input.result_is_base64,
            result_hash: input.result_hash,
            result_size: input.result_size,
            is_error: input.is_error,
            called_at: input.called_at,
        },
    )
}

pub fn decode_called(record: &EventRecord) -> Result<(String, String, RecordedCall)> {
    let e: Called = decode_event(record)?;
    let call_key = call_key_for(&e.connection, &e.tool, &e.args_json_redacted)?;
    Ok((
        e.app,
        call_key,
        RecordedCall {
            connection: e.connection,
            tool: e.tool,
            args_json_redacted: e.args_json_redacted,
            result_kind: e.result_kind,
            result: e.result,
            result_is_base64: e.result_is_base64,
            result_hash: e.result_hash,
            result_size: e.result_size,
            is_error: e.is_error,
            called_at: e.called_at,
        },
    ))
}

pub fn mcp_resource_id(name: &str) -> Result<String> {
    Ok(format!("mcp:{}", validate_name(name)?))
}

pub fn validate_name(name: &str) -> Result<String> {
    terrane_cap_connection::validate_name(name)
}

pub fn prepare_transport(raw: &str) -> Result<String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("mcp transport must be JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("mcp transport must be a JSON object".into()))?;
    let has_stdio = obj.contains_key("stdio");
    let has_http = obj.contains_key("http");
    if has_stdio == has_http {
        return Err(Error::InvalidInput(
            "mcp transport must contain exactly one of stdio or http".into(),
        ));
    }
    if has_stdio {
        validate_stdio(obj.get("stdio").unwrap_or(&Value::Null))?;
    } else {
        validate_http(obj.get("http").unwrap_or(&Value::Null))?;
    }
    let redacted = redact_transport(value);
    canonical_json(&redacted)
}

pub fn prepare_call(connection: &str, tool: &str, args_json: &str) -> Result<PreparedCall> {
    let connection = validate_name(connection)?;
    let tool = validate_tool(tool)?;
    if args_json.len() > MAX_ARGS_BYTES {
        return Err(Error::InvalidInput(format!(
            "mcp args exceed {MAX_ARGS_BYTES} bytes"
        )));
    }
    let mut value: Value = serde_json::from_str(args_json)
        .map_err(|e| Error::InvalidInput(format!("mcp args_json must be JSON object: {e}")))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("mcp args_json must be a JSON object".into()))?;
    let timeout_ms = parse_timeout(obj.remove("timeoutMs").as_ref())?;
    let sensitive = parse_sensitive_args(obj.remove("sensitiveArgs").as_ref())?;
    let canonical_args = canonical_json(&value)?;
    let mut redacted = value.clone();
    for pointer in sensitive {
        redact_pointer(&mut redacted, &pointer)?;
    }
    let args_json_redacted = canonical_json(&redacted)?;
    let call_key = call_key_for(&connection, &tool, &canonical_args)?;
    Ok(PreparedCall {
        connection,
        tool,
        args_json: canonical_args,
        args_json_redacted,
        call_key,
        timeout_ms,
    })
}

pub fn call_key_for(connection: &str, tool: &str, args_json: &str) -> Result<String> {
    let value = serde_json::json!({
        "connection": connection,
        "tool": tool,
        "args": serde_json::from_str::<Value>(args_json)
            .map_err(|e| Error::InvalidInput(format!("call key args JSON: {e}")))?,
    });
    Ok(sha256_hex(canonical_json(&value)?.as_bytes()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct McpClientCapability;

impl Capability for McpClientCapability {
    fn namespace(&self) -> &'static str {
        "mcp"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "mcp.connect" },
                CommandSpec {
                    name: "mcp.disconnect",
                },
                CommandSpec { name: "mcp.call" },
                CommandSpec { name: "mcp.tools" },
            ],
            events: vec![
                EventSpec {
                    kind: "mcp.connected",
                },
                EventSpec {
                    kind: "mcp.disconnected",
                },
                EventSpec { kind: "mcp.called" },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "call",
                    params: &["connection", "tool", "argsJson"],
                },
                ResourceMethod::Call {
                    name: "tools",
                    params: &["connection"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "mcp",
                &["call"],
                "Per-server external MCP tool calls through mcp:<name> grants.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::mcp_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "mcp.connect" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                let transport = prepare_transport(&arg(args, 1, "transport_json")?)?;
                let state = state_ref::<McpClientState>(ctx.state, "mcp")?;
                if !state.connections.contains_key(&conn_name)
                    && state.connections.len() >= MAX_CONNECTIONS
                {
                    return Err(Error::InvalidInput(format!(
                        "mcp connection limit exceeded: max {MAX_CONNECTIONS}"
                    )));
                }
                Ok(Decision::Commit(vec![connected_event(&conn_name, &transport)?]))
            }
            "mcp.disconnect" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                Ok(Decision::Commit(vec![disconnected_event(&conn_name)?]))
            }
            "mcp.call" | "mcp.tools" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                let connection = validate_name(&arg(args, 1, "connection")?)?;
                ensure_registered(ctx.state, &connection)?;
                ensure_mcp_grant(ctx.state, &app, &connection)?;
                if name == "mcp.tools" {
                    return Ok(Decision::TransientEffect(Effect::McpTools {
                        app,
                        connection,
                    }));
                }
                let tool = arg(args, 2, "tool")?;
                let args_json = arg(args, 3, "args_json")?;
                let prepared = prepare_call(&connection, &tool, &args_json)?;
                Ok(Decision::Effect(Effect::McpCall {
                    app,
                    connection: prepared.connection,
                    tool: prepared.tool,
                    args: prepared.args_json,
                    args_redacted: prepared.args_json_redacted,
                    timeout_ms: prepared.timeout_ms,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "mcp.connected" => {
                let e: Connected = decode_event(record)?;
                state_mut::<McpClientState>(state, "mcp")?
                    .connections
                    .insert(e.name, e.transport_json_redacted);
            }
            "mcp.disconnected" => {
                let e: Disconnected = decode_event(record)?;
                state_mut::<McpClientState>(state, "mcp")?
                    .connections
                    .remove(&e.name);
            }
            "mcp.called" => {
                let (app, call_key, call) = decode_called(record)?;
                state_mut::<McpClientState>(state, "mcp")?
                    .calls
                    .entry(app)
                    .or_default()
                    .insert(call_key, call);
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<McpClientState>(state, "mcp")?
                    .calls
                    .remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "mcp.connected" => decode_event::<Connected>(record)
                .ok()
                .map(|e| format!("mcp.connected {}", e.name)),
            "mcp.disconnected" => decode_event::<Disconnected>(record)
                .ok()
                .map(|e| format!("mcp.disconnected {}", e.name)),
            "mcp.called" => decode_event::<Called>(record).ok().map(|e| {
                format!(
                    "mcp.called {} {} size={} error={}",
                    e.connection, e.tool, e.result_size, e.is_error
                )
            }),
            _ => None,
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
            "call" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "mcp.called")
                    .ok_or_else(|| Error::Runtime("mcp.call produced no recorded result".into()))?;
                let e: Called = decode_event(record)?;
                if e.result_kind == "inline" {
                    return Ok(ReadValue::OptString(Some(e.result)));
                }
                Err(Error::Runtime(format!(
                    "mcp.call result is in blob __mcp__/{}",
                    call_key_for(&e.connection, &e.tool, &e.args_json_redacted)?
                )))
            }
            "tools" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "mcp.called")
                    .ok_or_else(|| Error::Runtime("mcp.tools produced no transient result".into()))?;
                let e: Called = decode_event(record)?;
                Ok(ReadValue::OptString(Some(e.result)))
            }
            other => Err(Error::InvalidInput(format!(
                "mcp.{other} is not a callable resource"
            ))),
        }
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        (method == "call").then_some(RecordedCallCap {
            limit: MAX_CALLS_PER_RUN,
            escape_hint: "external MCP calls are recorded; split large loops across backend runs",
        })
    }
}

fn ensure_registered(state: &dyn StateStore, connection: &str) -> Result<()> {
    if state_ref::<McpClientState>(state, "mcp")?
        .connections
        .contains_key(connection)
    {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "unknown mcp connection: {connection}"
    )))
}

fn ensure_mcp_grant(state: &dyn StateStore, app: &str, connection: &str) -> Result<()> {
    let resource_id = mcp_resource_id(connection)?;
    let principal = ExecutionPrincipal::local_owner();
    if terrane_cap_auth::resource_granted(state, &principal, app, &resource_id)? {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "permission required: grant {resource_id} to {app} for {LOCAL_OWNER_SUBJECT}"
    )))
}

fn validate_stdio(value: &Value) -> Result<()> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("stdio transport must be an object".into()))?;
    let cmd = required_string(obj, "cmd")?;
    if cmd.trim().is_empty() {
        return Err(Error::InvalidInput("stdio cmd must not be empty".into()));
    }
    if let Some(args) = obj.get("args") {
        let args = args
            .as_array()
            .ok_or_else(|| Error::InvalidInput("stdio args must be an array".into()))?;
        for arg in args {
            let _ = arg
                .as_str()
                .ok_or_else(|| Error::InvalidInput("stdio args items must be strings".into()))?;
        }
    }
    if let Some(env) = obj.get("env") {
        let env = env
            .as_object()
            .ok_or_else(|| Error::InvalidInput("stdio env must be an object".into()))?;
        for (name, value) in env {
            if name.trim().is_empty() {
                return Err(Error::InvalidInput("stdio env names must not be empty".into()));
            }
            validate_string_or_secret(value, "stdio env value")?;
        }
    }
    Ok(())
}

fn validate_http(value: &Value) -> Result<()> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("http transport must be an object".into()))?;
    let url = required_string(obj, "url")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::InvalidInput(
            "http mcp transport URL must be http or https".into(),
        ));
    }
    if let Some(headers) = obj.get("headers") {
        let headers = headers
            .as_object()
            .ok_or_else(|| Error::InvalidInput("http headers must be an object".into()))?;
        for (name, value) in headers {
            if name.trim().is_empty() {
                return Err(Error::InvalidInput("http header names must not be empty".into()));
            }
            validate_string_or_secret(value, "http header value")?;
        }
    }
    Ok(())
}

fn required_string(obj: &Map<String, Value>, name: &str) -> Result<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
}

fn validate_string_or_secret(value: &Value, label: &str) -> Result<()> {
    if value.is_string() || parse_secret(value)?.is_some() {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "{label} must be a string or {{\"$secret\":\"name\"}}"
    )))
}

fn parse_secret(value: &Value) -> Result<Option<String>> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if obj.len() == 1 {
        if let Some(secret) = obj.get("$secret") {
            let reference = secret
                .as_str()
                .ok_or_else(|| Error::InvalidInput("$secret value must be a string".into()))?;
            let _ = terrane_cap_connection::split_secret_ref(reference)?;
            return Ok(Some(reference.to_string()));
        }
    }
    Ok(None)
}

fn redact_transport(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(redact_transport).collect()),
        Value::Object(obj) => {
            if obj.len() == 1 && obj.contains_key("$secret") {
                return Value::Object(obj);
            }
            let mut parent = Map::new();
            for (key, value) in obj {
                let key_lower = key.to_ascii_lowercase();
                let redacted = if matches!(
                    key_lower.as_str(),
                    "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
                ) || key_lower.ends_with("-token")
                    || key_lower.ends_with("-secret")
                    || key_lower == "api-key"
                    || key_lower == "x-api-key"
                    || key_lower == "env"
                {
                    redact_transport_leaf(value)
                } else {
                    redact_transport(value)
                };
                parent.insert(key, redacted);
            }
            Value::Object(parent)
        }
        other => other,
    }
}

fn redact_transport_leaf(value: Value) -> Value {
    match value {
        Value::Object(obj) => {
            if obj.len() == 1 && obj.contains_key("$secret") {
                return Value::Object(obj);
            }
            Value::String(REDACTED.to_string())
        }
        Value::Array(_) => Value::String(REDACTED.to_string()),
        Value::String(_) => Value::String(REDACTED.to_string()),
        other => other,
    }
}

fn validate_tool(tool: &str) -> Result<String> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err(Error::InvalidInput("mcp tool must not be empty".into()));
    }
    Ok(tool.to_string())
}

fn parse_timeout(value: Option<&Value>) -> Result<u64> {
    let Some(value) = value else {
        return Ok(DEFAULT_TIMEOUT_MS);
    };
    let timeout = value
        .as_u64()
        .ok_or_else(|| Error::InvalidInput("timeoutMs must be a positive integer".into()))?;
    if timeout == 0 || timeout > MAX_TIMEOUT_MS {
        return Err(Error::InvalidInput(format!(
            "timeoutMs must be 1..{MAX_TIMEOUT_MS}"
        )));
    }
    Ok(timeout)
}

fn parse_sensitive_args(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("sensitiveArgs must be an array".into()))?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let pointer = item
            .as_str()
            .ok_or_else(|| Error::InvalidInput("sensitiveArgs items must be strings".into()))?;
        if !pointer.starts_with('/') {
            return Err(Error::InvalidInput(
                "sensitiveArgs items must be JSON pointers".into(),
            ));
        }
        out.push(pointer.to_string());
    }
    Ok(out)
}

fn redact_pointer(value: &mut Value, pointer: &str) -> Result<()> {
    match value.pointer_mut(pointer) {
        Some(slot) => {
            *slot = Value::String(REDACTED.to_string());
            Ok(())
        }
        None => Err(Error::InvalidInput(format!(
            "sensitiveArgs pointer not found: {pointer}"
        ))),
    }
}

fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&sort_json(value))
        .map_err(|e| Error::InvalidInput(format!("canonicalize mcp JSON: {e}")))
}

fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort_json).collect()),
        Value::Object(obj) => {
            let mut sorted = Map::new();
            let mut keys: Vec<_> = obj.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(value) = obj.get(key) {
                    sorted.insert(key.clone(), sort_json(value));
                }
            }
            Value::Object(sorted)
        }
        other => other.clone(),
    }
}
