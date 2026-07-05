use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use terrane_api::MCP_PROTOCOL_VERSION;
use terrane_core::{Error, EventRecord, Result, State};

const TOOLS_LIST: &str = "__tools/list";

pub struct McpCallRequest<'a> {
    pub app: &'a str,
    pub connection: &'a str,
    pub tool: &'a str,
    pub args: &'a str,
    pub args_redacted: &'a str,
    pub timeout_ms: u64,
}

pub fn call(home: &Path, state: &State, request: McpCallRequest<'_>) -> Result<Vec<EventRecord>> {
    let result = match call_inner(
        home,
        state,
        request.connection,
        request.tool,
        request.args,
        request.timeout_ms,
    ) {
        Ok(result) => result,
        Err(err) => McpResult {
            content: serde_json::json!([{"type":"text","text":err.to_string()}]),
            is_error: true,
        },
    };
    recorded_result(
        home,
        request.app,
        request.connection,
        request.tool,
        request.args_redacted,
        result,
    )
}

pub fn list_tools(home: &Path, state: &State, app: &str, connection: &str) -> Result<Vec<EventRecord>> {
    let result = match call_inner(home, state, connection, TOOLS_LIST, "{}", terrane_cap_mcp_client::DEFAULT_TIMEOUT_MS) {
        Ok(result) => result,
        Err(err) => McpResult {
            content: serde_json::json!({"tools":[],"error":err.to_string()}),
            is_error: true,
        },
    };
    recorded_result(home, app, connection, TOOLS_LIST, "{}", result)
}

fn call_inner(
    home: &Path,
    state: &State,
    connection: &str,
    tool: &str,
    args: &str,
    timeout_ms: u64,
) -> Result<McpResult> {
    let transport = state
        .mcp
        .connections
        .get(connection)
        .ok_or_else(|| Error::InvalidInput(format!("unknown mcp connection: {connection}")))?;
    let mut value: Value = serde_json::from_str(transport)
        .map_err(|e| Error::InvalidInput(format!("mcp transport JSON is invalid: {e}")))?;
    resolve_transport_secrets(home, &mut value)?;
    if let Some(stdio) = value.get("stdio") {
        call_stdio(stdio, tool, args, timeout_ms)
    } else if let Some(http) = value.get("http") {
        call_http(http, tool, args, timeout_ms)
    } else {
        Err(Error::InvalidInput(
            "mcp transport must contain stdio or http".into(),
        ))
    }
}

fn recorded_result(
    home: &Path,
    app: &str,
    connection: &str,
    tool: &str,
    args_json_redacted: &str,
    result: McpResult,
) -> Result<Vec<EventRecord>> {
    let result_json = canonical_json(&result.content)?;
    let bytes = result_json.as_bytes();
    if bytes.len() > terrane_cap_mcp_client::RESULT_HARD_LIMIT {
        return Err(Error::Storage(format!(
            "mcp result exceeds {} bytes",
            terrane_cap_mcp_client::RESULT_HARD_LIMIT
        )));
    }
    let hash = terrane_cap_mcp_client::sha256_hex(bytes);
    let size = u64::try_from(bytes.len())
        .map_err(|_| Error::Storage("mcp result length overflow".into()))?;
    let mut records = Vec::new();
    let (kind, recorded) = if bytes.len() <= terrane_cap_mcp_client::INLINE_RESULT_LIMIT {
        ("inline", result_json)
    } else {
        crate::blob_store::insert_if_absent(home, &hash, bytes)?;
        let call_key = terrane_cap_mcp_client::call_key_for(connection, tool, args_json_redacted)?;
        records.push(terrane_cap_blob::stored_event(
            app,
            format!("__mcp__/{call_key}"),
            &hash,
            size,
            "application/json",
        )?);
        ("blob", String::new())
    };
    records.push(terrane_cap_mcp_client::called_event(
        terrane_cap_mcp_client::CalledEvent {
            app: app.to_string(),
            connection: connection.to_string(),
            tool: tool.to_string(),
            args_json_redacted: args_json_redacted.to_string(),
            result_kind: kind.to_string(),
            result: recorded,
            result_is_base64: false,
            result_hash: hash,
            result_size: size,
            is_error: result.is_error,
            called_at: called_at()?,
        },
    )?);
    Ok(records)
}

#[derive(Debug)]
struct McpResult {
    content: Value,
    is_error: bool,
}

fn call_stdio(transport: &Value, tool: &str, args: &str, timeout_ms: u64) -> Result<McpResult> {
    let obj = transport
        .as_object()
        .ok_or_else(|| Error::InvalidInput("stdio transport must be an object".into()))?;
    let cmd = string_field(obj, "cmd")?;
    let mut command = Command::new(&cmd);
    if let Some(args) = obj.get("args").and_then(Value::as_array) {
        for arg in args {
            command.arg(
                arg.as_str()
                    .ok_or_else(|| Error::InvalidInput("stdio args items must be strings".into()))?,
            );
        }
    }
    if let Some(env) = obj.get("env").and_then(Value::as_object) {
        for (name, value) in env {
            command.env(name, value_to_string(value)?);
        }
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| Error::Storage(format!("spawn mcp stdio `{cmd}`: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Storage("mcp stdio child stdin unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Storage("mcp stdio child stdout unavailable".into()))?;
    let mut reader = BufReader::new(stdout);

    write_json_line(&mut stdin, initialize_request(1)?)?;
    let _ = read_json_line(&mut reader, timeout_ms)?;
    write_json_line(&mut stdin, initialized_notification()?)?;
    let request = if tool == TOOLS_LIST {
        tools_list_request(2)?
    } else {
        tools_call_request(2, tool, args)?
    };
    write_json_line(&mut stdin, request)?;
    let response = read_json_line(&mut reader, timeout_ms)?;
    let _ = child.kill();
    let _ = child.wait();
    parse_response(response, tool)
}

fn call_http(transport: &Value, tool: &str, args: &str, timeout_ms: u64) -> Result<McpResult> {
    let obj = transport
        .as_object()
        .ok_or_else(|| Error::InvalidInput("http transport must be an object".into()))?;
    let url = string_field(obj, "url")?;
    validate_http_target(&url)?;
    let timeout = Duration::from_millis(timeout_ms);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .redirects(0)
        .build();
    let body = if tool == TOOLS_LIST {
        tools_list_request(1)?
    } else {
        tools_call_request(1, tool, args)?
    };
    let mut req = agent
        .post(&url)
        .set("content-type", "application/json")
        .set("accept", "application/json");
    if let Some(headers) = obj.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            req = req.set(name, &value_to_string(value)?);
        }
    }
    let resp = match req.send_string(&body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(ureq::Error::Transport(transport)) => {
            return Err(Error::Storage(format!("mcp http {url} failed: {transport}")))
        }
    };
    let value: Value = serde_json::from_reader(resp.into_reader())
        .map_err(|e| Error::Storage(format!("read mcp http response: {e}")))?;
    parse_response(value, tool)
}

fn parse_response(value: Value, tool: &str) -> Result<McpResult> {
    if let Some(error) = value.get("error") {
        return Ok(McpResult {
            content: serde_json::json!([{"type":"text","text":canonical_json(error)?}]),
            is_error: true,
        });
    }
    let result = value
        .get("result")
        .cloned()
        .ok_or_else(|| Error::Storage("mcp response missing result".into()))?;
    if tool == TOOLS_LIST {
        return Ok(McpResult {
            content: result,
            is_error: false,
        });
    }
    let content = result
        .get("content")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(McpResult { content, is_error })
}

fn initialize_request(id: u64) -> Result<String> {
    canonical_json(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "terrane", "version": "0.1.0"}
        }
    }))
}

fn initialized_notification() -> Result<String> {
    canonical_json(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }))
}

fn tools_list_request(id: u64) -> Result<String> {
    canonical_json(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/list",
        "params": {}
    }))
}

fn tools_call_request(id: u64, tool: &str, args: &str) -> Result<String> {
    let arguments: Value = serde_json::from_str(args)
        .map_err(|e| Error::InvalidInput(format!("mcp tool args JSON: {e}")))?;
    canonical_json(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    }))
}

fn write_json_line(stdin: &mut impl Write, text: String) -> Result<()> {
    stdin
        .write_all(text.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|e| Error::Storage(format!("write mcp stdio request: {e}")))
}

fn read_json_line(reader: &mut impl BufRead, _timeout_ms: u64) -> Result<Value> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| Error::Storage(format!("read mcp stdio response: {e}")))?;
    if line.trim().is_empty() {
        return Err(Error::Storage("mcp stdio returned empty response".into()));
    }
    serde_json::from_str(&line).map_err(|e| Error::Storage(format!("parse mcp stdio JSON: {e}")))
}

fn resolve_transport_secrets(home: &Path, value: &mut Value) -> Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                resolve_transport_secrets(home, item)?;
            }
        }
        Value::Object(obj) => {
            if obj.len() == 1 {
                if let Some(secret) = obj.get("$secret") {
                    let reference = secret
                        .as_str()
                        .ok_or_else(|| Error::InvalidInput("$secret value must be a string".into()))?;
                    let (name, field) = terrane_cap_connection::split_secret_ref(reference)?;
                    *value = Value::String(crate::secret_store::get_secret(home, &name, &field)?);
                    return Ok(());
                }
            }
            for item in obj.values_mut() {
                resolve_transport_secrets(home, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn string_field(obj: &Map<String, Value>, name: &str) -> Result<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
}

fn value_to_string(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput("mcp header/env values must resolve to strings".into()))
}

fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&sort_json(value))
        .map_err(|e| Error::InvalidInput(format!("serialize MCP JSON: {e}")))
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

fn called_at() -> Result<String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Storage(format!("system clock before Unix epoch: {e}")))?;
    Ok(format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis()))
}

fn validate_http_target(url: &str) -> Result<()> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::InvalidInput("mcp http URL must include http:// or https://".into()))?;
    if !matches!(scheme, "http" | "https") {
        return Err(Error::InvalidInput("mcp http URL must be http or https".into()));
    }
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidInput("mcp http URL missing host".into()))?;
    let (host, port) = split_host_port(host_port, scheme)?;
    if host == "169.254.169.254" {
        return Err(Error::InvalidInput(
            "mcp http to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        deny_metadata_ip(ip)?;
        return Ok(());
    }
    for addr in (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Storage(format!("resolve {host}: {e}")))?
    {
        deny_metadata_ip(addr.ip())?;
    }
    Ok(())
}

fn split_host_port(host_port: &str, scheme: &str) -> Result<(String, u16)> {
    let default_port = if scheme == "https" { 443 } else { 80 };
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| Error::InvalidInput("invalid bracketed IPv6 URL host".into()))?;
        let port = if let Some(port) = tail.strip_prefix(':') {
            parse_port(port)?
        } else {
            default_port
        };
        return Ok((host.to_string(), port));
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host.to_string(), parse_port(port)?)),
        _ => Ok((host_port.to_string(), default_port)),
    }
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse::<u16>()
        .map_err(|_| Error::InvalidInput(format!("invalid URL port: {port}")))
}

fn deny_metadata_ip(ip: IpAddr) -> Result<()> {
    if ip == IpAddr::from([169, 254, 169, 254]) {
        return Err(Error::InvalidInput(
            "mcp http to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    Ok(())
}
