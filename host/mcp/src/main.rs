//! terrane-mcp — a stdio MCP server exposing this terrane home's apps as tools.
//!
//! A thin host over the `terrane_cli`/`terrane-core` spine, like the CLI and web
//! hosts. It speaks the Model Context Protocol over newline-delimited JSON-RPC on
//! stdin/stdout so an MCP client (e.g. Claude Code) can **select an app**
//! (`list_apps`) and **act on it** (`invoke`). Both tools and their shapes are
//! the contract in [`terrane_api`].
//!
//! Everything is single-threaded and synchronous: one `Core` over `$TERRANE_HOME`,
//! one message at a time — which suits both the non-`Send` `Core` and the stdio
//! transport. stdout is reserved for protocol frames; all logging goes to stderr.

use std::io::{BufRead, Write};

use nanoserde::{DeJson, SerJson};
use terrane_api::{mcp_tools, AppSummary, AppsResponse, MCP_PROTOCOL_VERSION, TOOL_INVOKE, TOOL_LIST_APPS};
use terrane_cli::EdgeRunner;
use terrane_core::Core;
use terrane_domain::Request;

fn main() {
    let mut core = match open_core() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-mcp: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("terrane-mcp: ready (home {})", terrane_cli::log_path().display());

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF — client disconnected.
            Ok(_) => {}
            Err(e) => {
                eprintln!("terrane-mcp: read error: {e}");
                break;
            }
        }
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        if let Some(response) = handle(&mut core, raw) {
            if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    }
}

/// Open the home's core and ensure it has minted its replica identity (so CRDT
/// apps author under a stable peer).
fn open_core() -> Result<Core<EdgeRunner>, String> {
    let mut core = Core::open_with(terrane_cli::log_path(), EdgeRunner).map_err(|e| e.to_string())?;
    if core.state().replica.peer.is_none() {
        core.dispatch(Request::new("replica.init", Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    Ok(core)
}

/// The fields we need off any incoming JSON-RPC message. `method` parses via
/// nanoserde (unknown fields are skipped); `id` is extracted raw so we can echo
/// it back verbatim regardless of whether it's a number or a string.
#[derive(DeJson)]
struct Rpc {
    #[nserde(default)]
    method: String,
}

/// Route one message. Returns the response line, or `None` for notifications
/// (no `id`) and messages we don't answer.
fn handle(core: &mut Core<EdgeRunner>, raw: &str) -> Option<String> {
    let rpc: Rpc = match DeJson::deserialize_json(raw) {
        Ok(rpc) => rpc,
        Err(e) => {
            eprintln!("terrane-mcp: ignoring unparseable message: {e}");
            return None;
        }
    };
    let id = extract_id(raw);
    match rpc.method.as_str() {
        "initialize" => id.map(|id| ok(&id, &initialize_result())),
        "ping" => id.map(|id| ok(&id, "{}")),
        "tools/list" => id.map(|id| ok(&id, &tools_list_result())),
        "tools/call" => id.map(|id| tool_call(core, &id, raw)),
        // Notifications (no id) and anything else we don't implement.
        other => id.map(|id| error(&id, -32601, &format!("method not found: {other}"))),
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
struct CallWrap {
    params: CallParams,
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

fn tool_call(core: &mut Core<EdgeRunner>, id: &str, raw: &str) -> String {
    let call: CallWrap = match DeJson::deserialize_json(raw) {
        Ok(call) => call,
        Err(e) => return error(id, -32602, &format!("invalid params: {e}")),
    };
    let args = call.params.arguments;
    match call.params.name.as_str() {
        TOOL_LIST_APPS => tool_text(id, &list_apps(core), false),
        TOOL_INVOKE => {
            if args.app.is_empty() || args.verb.is_empty() {
                return tool_text(id, "invoke requires non-empty 'app' and 'verb'", true);
            }
            match invoke(core, &args.app, &args.verb, &args.args) {
                Ok(output) => tool_text(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        other => error(id, -32602, &format!("unknown tool: {other}")),
    }
}

/// `list_apps` → the catalog as JSON text (so the agent can parse it).
fn list_apps(core: &mut Core<EdgeRunner>) -> String {
    let apps: Vec<AppSummary> = core
        .state()
        .app
        .apps
        .values()
        .map(|app| AppSummary {
            id: app.id.clone(),
            name: app.name.clone(),
            has_ui: app_has_ui(app.source.as_deref()),
        })
        .collect();
    AppsResponse { apps }.serialize_json()
}

/// Whether the app's bundle declares a UI (`manifest.ui`).
fn app_has_ui(source: Option<&str>) -> bool {
    let Some(source) = source else {
        return false;
    };
    let manifest = std::path::Path::new(source).join("manifest.json");
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    #[derive(DeJson)]
    struct Manifest {
        #[nserde(default)]
        ui: String,
    }
    Manifest::deserialize_json(&text)
        .map(|m| !m.ui.is_empty())
        .unwrap_or(false)
}

/// `invoke` → run `host.run app verb [args…]` and return the backend's string.
fn invoke(core: &mut Core<EdgeRunner>, app: &str, verb: &str, args: &[String]) -> Result<String, String> {
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push(app.to_string());
    argv.push(verb.to_string());
    argv.extend(args.iter().cloned());
    core.dispatch(Request::new("host.run", argv))
        .map_err(|e| e.to_string())?;
    Ok(core.take_last_output().unwrap_or_default())
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

/// Pull the raw `id` token (number or quoted string) out of a JSON-RPC message so
/// we can echo it back verbatim. Returns `None` for notifications (no top-level
/// `id`) or a `null` id. Tolerant of optional space after the key.
fn extract_id(raw: &str) -> Option<String> {
    let key = raw.find("\"id\"")?;
    let after_key = raw[key + 4..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let token = if let Some(rest) = after_colon.strip_prefix('"') {
        // String id: take through the closing quote (minimal escape handling).
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        format!("\"{}\"", &rest[..i.min(rest.len())])
    } else {
        // Number id: up to the next delimiter.
        let end = after_colon
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(after_colon.len());
        after_colon[..end].to_string()
    };
    if token.is_empty() || token == "null" {
        None
    } else {
        Some(token)
    }
}
