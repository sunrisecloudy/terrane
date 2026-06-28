//! Shared MCP semantics for Terrane host transports.
//!
//! Stdio and HTTP are just transports. This module owns the JSON-RPC request
//! handling and MCP tool behavior so every host exposes the same list ->
//! discover -> act contract.

use nanoserde::{DeJson, SerJson};
use terrane_api::{mcp_tools, MCP_PROTOCOL_VERSION, TOOL_APP_ACTIONS, TOOL_INVOKE, TOOL_LIST_APPS};

use crate::HostCore;

/// Route one JSON-RPC message. Returns a response JSON string, or `None` for
/// notifications (no `id`) and messages that do not require an answer.
///
/// The JSON-RPC envelope is parsed structurally (top-level object only), not by
/// substring scan or whole-message nanoserde: `id` is captured as its exact
/// source token so it echoes back verbatim whatever its type.
pub fn handle_json_rpc(core: &mut HostCore, raw: &str) -> Option<String> {
    let fields = top_level_fields(raw);
    let field = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

    // A present, non-null top-level id means a request that needs a response.
    let id = field("id").filter(|v| *v != "null");
    let method = field("method").and_then(json_string_value);

    match method {
        Some("initialize") => id.map(|id| ok(id, &initialize_result())),
        Some("ping") => id.map(|id| ok(id, "{}")),
        Some("tools/list") => id.map(|id| ok(id, &tools_list_result())),
        Some("tools/call") => id.map(|id| tool_call(core, id, field("params").unwrap_or("{}"))),
        // Notifications (no id) are silently accepted; a request with an
        // unknown/!string method still gets a proper error reply.
        _ => id.map(|id| {
            let shown = field("method").unwrap_or("(none)");
            error(id, -32601, &format!("method not found: {shown}"))
        }),
    }
}

fn initialize_result() -> String {
    format!(
        r#"{{"protocolVersion":{},"capabilities":{{"tools":{{"listChanged":false}}}},"serverInfo":{{"name":"terrane-mcp","version":"0.1.0"}}}}"#,
        json_str(MCP_PROTOCOL_VERSION)
    )
}

fn tools_list_result() -> String {
    let tools: Vec<String> = mcp_tools()
        .iter()
        .map(|t| {
            format!(
                r#"{{"name":{},"description":{},"inputSchema":{}}}"#,
                json_str(t.name),
                json_str(t.description),
                t.input_schema // already a JSON object literal
            )
        })
        .collect();
    format!(r#"{{"tools":[{}]}}"#, tools.join(","))
}

#[derive(DeJson)]
struct CallParams {
    name: String,
    #[nserde(default)]
    arguments: CallArgs,
}

#[derive(DeJson, Default)]
struct CallArgs {
    #[nserde(default)]
    app: String,
    #[nserde(default)]
    verb: String,
    #[nserde(default)]
    args: Vec<String>,
}

/// Handle `tools/call`. `params_raw` is the isolated top-level `params` object,
/// so nanoserde only ever parses that sub-object (never the whole envelope).
fn tool_call(core: &mut HostCore, id: &str, params_raw: &str) -> String {
    let params: CallParams = match DeJson::deserialize_json(params_raw) {
        Ok(params) => params,
        Err(e) => return error(id, -32602, &format!("invalid params: {e}")),
    };
    let args = params.arguments;
    match params.name.as_str() {
        TOOL_LIST_APPS => tool_text(id, &crate::list_apps(core).serialize_json(), false),
        TOOL_APP_ACTIONS => {
            if args.app.is_empty() {
                return tool_text(id, "app_actions requires non-empty 'app'", true);
            }
            match crate::app_actions(core, &args.app) {
                Ok(output) => tool_text(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_INVOKE => {
            if args.app.is_empty() || args.verb.is_empty() {
                return tool_text(id, "invoke requires non-empty 'app' and 'verb'", true);
            }
            match crate::invoke_app(core, &args.app, &args.verb, &args.args) {
                Ok(output) => tool_text(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        other => error(id, -32602, &format!("unknown tool: {other}")),
    }
}

// --- JSON-RPC framing helpers -------------------------------------------------

fn ok(id: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result_json}}}"#)
}

fn error(id: &str, code: i64, message: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{}}}}}"#,
        json_str(message)
    )
}

/// An MCP tool result: a single text content block, with an error flag the model
/// can see (tool failures are results, not protocol errors).
fn tool_text(id: &str, text: &str, is_error: bool) -> String {
    let result = format!(
        r#"{{"content":[{{"type":"text","text":{}}}],"isError":{is_error}}}"#,
        json_str(text)
    );
    ok(id, &result)
}

/// Quote + escape a string as JSON (reusing nanoserde's `String` serializer).
fn json_str(s: &str) -> String {
    s.to_string().serialize_json()
}

/// Parse the top-level object of a JSON message into `(key, raw_value)` pairs,
/// where each value is its exact source slice (balanced for objects/arrays/
/// strings). Only depth-1 keys are returned; nested keys are ignored. Empty if
/// the input isn't a JSON object.
fn top_level_fields(raw: &str) -> Vec<(&str, &str)> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = skip_ws(bytes, 0);
    if bytes.get(i) != Some(&b'{') {
        return out;
    }
    i += 1;
    loop {
        i = skip_ws(bytes, i);
        match bytes.get(i) {
            Some(b'"') => {}
            _ => return out, // '}' (done) or malformed
        }
        let Some(key_end) = scan_string_end(bytes, i) else {
            return out;
        };
        let key = &raw[i + 1..key_end - 1]; // inner (our keys are escape-free)
        i = skip_ws(bytes, key_end);
        if bytes.get(i) != Some(&b':') {
            return out;
        }
        i = skip_ws(bytes, i + 1);
        let Some(value_end) = scan_value(bytes, i) else {
            return out;
        };
        out.push((key, &raw[i..value_end]));
        i = skip_ws(bytes, value_end);
        match bytes.get(i) {
            Some(b',') => i += 1,
            _ => return out, // '}' (done) or malformed
        }
    }
}

/// The inner text of a JSON string token (`"x"` -> `x`), or `None` if `raw`
/// isn't a quoted string. Used for method names (escape-free), so no unescaping.
fn json_string_value(raw: &str) -> Option<&str> {
    raw.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// Index just past the closing quote of the string starting at `start`. Byte-safe
/// (UTF-8 continuation bytes are never `"` or `\`).
fn scan_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Index just past the JSON value starting at `start` (string, balanced
/// object/array, or primitive). `None` if it runs off the end.
fn scan_value(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => scan_string_end(bytes, start),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut i = start;
            let mut in_str = false;
            while i < bytes.len() {
                let c = bytes[i];
                if in_str {
                    match c {
                        b'\\' => i += 1,
                        b'"' => in_str = false,
                        _ => {}
                    }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1);
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = start;
            while i < bytes.len()
                && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                i += 1;
            }
            (i > start).then_some(i)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{json_string_value, top_level_fields};

    #[test]
    fn top_level_parser_ignores_nested_ids() {
        let raw = r#"{"jsonrpc":"2.0","method":"ping","params":{"item":{"id":555}},"id":8}"#;
        let fields = top_level_fields(raw);
        let field = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

        assert_eq!(field("id"), Some("8"));
        assert_eq!(field("method").and_then(json_string_value), Some("ping"));
        assert_eq!(field("params"), Some(r#"{"item":{"id":555}}"#));
    }
}
