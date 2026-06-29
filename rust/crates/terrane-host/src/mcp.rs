//! Shared MCP semantics for Terrane host transports.
//!
//! Stdio and HTTP are just transports. This module owns the JSON-RPC request
//! handling and MCP tool behavior so every host exposes the same list ->
//! discover -> act contract.

use nanoserde::SerJson;
use serde_json::{json, Map, Value};
use terrane_api::{
    mcp_tools, MCP_PROTOCOL_VERSION, TOOL_APP_ACTIONS, TOOL_APP_BUNDLE_VALIDATE, TOOL_APP_RECIPE,
    TOOL_APP_REGISTER, TOOL_APP_SCAFFOLD, TOOL_CAPABILITIES_LIST, TOOL_CAPABILITY_COMMAND,
    TOOL_CAPABILITY_INFO, TOOL_CAPABILITY_QUERY, TOOL_INVOKE, TOOL_LIST_APPS, TOOL_WORKFLOWS_LIST,
    TOOL_WORKFLOW_INFO,
};
use terrane_core::QueryValue;

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
            if let Some(tool) = method.filter(|tool| is_tool_name(tool)) {
                return error(
                    id,
                    -32601,
                    &format!(
                        "method not found: {shown}. MCP tools must be called through tools/call. Try: {}",
                        tool_call_example(tool)
                    ),
                );
            }
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

fn is_tool_name(name: &str) -> bool {
    mcp_tools().iter().any(|tool| tool.name == name)
}

struct CallParams {
    name: String,
    arguments: Value,
}

/// Handle `tools/call`. `params_raw` is the isolated top-level `params` object,
/// so argument failures can be surfaced as MCP tool errors instead of JSON-RPC
/// protocol errors.
fn tool_call(core: &mut HostCore, id: &str, params_raw: &str) -> String {
    let params = match parse_call_params(params_raw) {
        Ok(params) => params,
        Err(e) => return error(id, -32602, &e),
    };
    match params.name.as_str() {
        TOOL_WORKFLOWS_LIST => match args_object(TOOL_WORKFLOWS_LIST, &params.arguments) {
            Ok(_) => tool_json(id, &workflows_list_json(), false),
            Err(e) => tool_text(id, &e, true),
        },
        TOOL_WORKFLOW_INFO => {
            let args = match args_object(TOOL_WORKFLOW_INFO, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let name = match required_string(args, "name", TOOL_WORKFLOW_INFO) {
                Ok(name) => name,
                Err(e) => return tool_text(id, &e, true),
            };
            match workflow_info_json(&name) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_APP_RECIPE => {
            let args = match args_object(TOOL_APP_RECIPE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let kind = match optional_string(args, "kind", TOOL_APP_RECIPE) {
                Ok(kind) => kind,
                Err(e) => return tool_text(id, &e, true),
            };
            tool_json(id, &app_recipe_json(&kind), false)
        }
        TOOL_APP_SCAFFOLD => {
            let args = match args_object(TOOL_APP_SCAFFOLD, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let app_id = match required_string(args, "id", TOOL_APP_SCAFFOLD) {
                Ok(app_id) => app_id,
                Err(e) => return tool_text(id, &e, true),
            };
            let name = match required_string(args, "name", TOOL_APP_SCAFFOLD) {
                Ok(name) => name,
                Err(e) => return tool_text(id, &e, true),
            };
            let kind = match optional_string(args, "kind", TOOL_APP_SCAFFOLD) {
                Ok(kind) => kind,
                Err(e) => return tool_text(id, &e, true),
            };
            let with_ui = match optional_bool(args, "withUi", TOOL_APP_SCAFFOLD) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_scaffold_json(&app_id, &name, &kind, with_ui) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_APP_BUNDLE_VALIDATE => {
            let args = match args_object(TOOL_APP_BUNDLE_VALIDATE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let path = match required_string(args, "path", TOOL_APP_BUNDLE_VALIDATE) {
                Ok(path) => path,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_bundle_validate_json(&path) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_APP_REGISTER => {
            let args = match args_object(TOOL_APP_REGISTER, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let source = match required_string(args, "source", TOOL_APP_REGISTER) {
                Ok(source) => source,
                Err(e) => return tool_text(id, &e, true),
            };
            let id_override = match optional_string(args, "id", TOOL_APP_REGISTER) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let name_override = match optional_string(args, "name", TOOL_APP_REGISTER) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let runtime_override = match optional_string(args, "runtime", TOOL_APP_REGISTER) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let dry_run = match optional_bool(args, "dryRun", TOOL_APP_REGISTER) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_register_json(
                core,
                &source,
                &id_override,
                &name_override,
                &runtime_override,
                dry_run,
            ) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_LIST_APPS => match args_object(TOOL_LIST_APPS, &params.arguments) {
            Ok(_) => tool_json(id, &crate::list_apps(core).serialize_json(), false),
            Err(e) => tool_text(id, &e, true),
        },
        TOOL_APP_ACTIONS => {
            let args = match args_object(TOOL_APP_ACTIONS, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let app = match required_string(args, "app", TOOL_APP_ACTIONS) {
                Ok(app) => app,
                Err(e) => return tool_text(id, &e, true),
            };
            match crate::app_actions(core, &app) {
                Ok(output) => tool_json_if_possible(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_INVOKE => {
            let args = match args_object(TOOL_INVOKE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let app = match required_string(args, "app", TOOL_INVOKE) {
                Ok(app) => app,
                Err(e) => return tool_text(id, &e, true),
            };
            let verb = match required_string(args, "verb", TOOL_INVOKE) {
                Ok(verb) => verb,
                Err(e) => return tool_text(id, &e, true),
            };
            let argv = match optional_string_vec(args, "args", TOOL_INVOKE) {
                Ok(argv) => argv,
                Err(e) => return tool_text(id, &e, true),
            };
            match crate::invoke_app(core, &app, &verb, &argv) {
                Ok(output) => tool_text(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_CAPABILITIES_LIST => {
            let args = match args_object(TOOL_CAPABILITIES_LIST, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let include_internal =
                match optional_bool(args, "includeInternal", TOOL_CAPABILITIES_LIST) {
                    Ok(value) => value,
                    Err(e) => return tool_text(id, &e, true),
                };
            tool_json(
                id,
                &crate::cap_doc::capability_list_json(include_internal),
                false,
            )
        }
        TOOL_CAPABILITY_INFO => {
            let args = match args_object(TOOL_CAPABILITY_INFO, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let namespace = match required_string(args, "namespace", TOOL_CAPABILITY_INFO) {
                Ok(namespace) => namespace,
                Err(e) => return tool_text(id, &e, true),
            };
            let format = match optional_string(args, "format", TOOL_CAPABILITY_INFO) {
                Ok(format) => format,
                Err(e) => return tool_text(id, &e, true),
            };
            let include_internal =
                match optional_bool(args, "includeInternal", TOOL_CAPABILITY_INFO) {
                    Ok(value) => value,
                    Err(e) => return tool_text(id, &e, true),
                };
            match crate::cap_doc::render_capability_info(&namespace, &format, include_internal) {
                Ok(output) if format.trim().is_empty() || format.trim() == "json" => {
                    tool_json(id, &output, false)
                }
                Ok(output) => tool_text(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_CAPABILITY_QUERY => {
            let args = match args_object(TOOL_CAPABILITY_QUERY, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let capability = match required_string(args, "capability", TOOL_CAPABILITY_QUERY) {
                Ok(capability) => capability,
                Err(e) => return tool_text(id, &e, true),
            };
            let query = match required_string(args, "query", TOOL_CAPABILITY_QUERY) {
                Ok(query) => query,
                Err(e) => return tool_text(id, &e, true),
            };
            let argv = match optional_string_vec(args, "args", TOOL_CAPABILITY_QUERY) {
                Ok(argv) => argv,
                Err(e) => return tool_text(id, &e, true),
            };
            match crate::query_on_core(core, &capability, &query, &argv) {
                Ok(value) => tool_json(id, &query_value_json(&capability, &query, value), false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_CAPABILITY_COMMAND => {
            let args = match args_object(TOOL_CAPABILITY_COMMAND, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let name = match required_string(args, "name", TOOL_CAPABILITY_COMMAND) {
                Ok(name) => name,
                Err(e) => return tool_text(id, &e, true),
            };
            let argv = match optional_string_vec(args, "args", TOOL_CAPABILITY_COMMAND) {
                Ok(argv) => argv,
                Err(e) => return tool_text(id, &e, true),
            };
            let dry_run = match optional_bool(args, "dryRun", TOOL_CAPABILITY_COMMAND) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let help = match optional_bool(args, "help", TOOL_CAPABILITY_COMMAND) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            if help {
                return match crate::cap_doc::capability_command_help_json(&name) {
                    Ok(output) => tool_json(id, &output, false),
                    Err(e) => tool_text(id, &e, true),
                };
            }
            if dry_run {
                match crate::dry_run_on_core(core, &name, &argv) {
                    Ok(outcome) => tool_json(id, &command_dry_run_json(outcome.records), false),
                    Err(e) => tool_text(id, &e, true),
                }
            } else {
                match crate::dispatch_on_core(core, &name, &argv) {
                    Ok(outcome) => tool_json(
                        id,
                        &command_outcome_json(outcome.records.len(), outcome.output.as_deref()),
                        false,
                    ),
                    Err(e) => tool_text(id, &e, true),
                }
            }
        }
        other => tool_text(id, &format!("unknown tool: {other}"), true),
    }
}

fn parse_call_params(params_raw: &str) -> Result<CallParams, String> {
    let value: Value =
        serde_json::from_str(params_raw).map_err(|e| format!("invalid params: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "invalid params: tools/call params must be an object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "invalid params: tools/call requires string 'name'. Try: {}",
                tool_call_example(TOOL_WORKFLOW_INFO)
            )
        })?
        .to_string();
    let arguments = object
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    Ok(CallParams { name, arguments })
}

fn args_object<'a>(tool: &str, arguments: &'a Value) -> Result<&'a Map<String, Value>, String> {
    arguments.as_object().ok_or_else(|| {
        format!(
            "{tool} arguments must be an object. Try: {}",
            tool_call_example(tool)
        )
    })
}

fn required_string(args: &Map<String, Value>, field: &str, tool: &str) -> Result<String, String> {
    match args.get(field) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        Some(Value::String(_)) | None => Err(format!(
            "{tool} requires non-empty '{field}'. Try: {}",
            tool_call_example(tool)
        )),
        Some(_) => Err(format!(
            "{tool} requires string '{field}'. Try: {}",
            tool_call_example(tool)
        )),
    }
}

fn optional_string(args: &Map<String, Value>, field: &str, tool: &str) -> Result<String, String> {
    match args.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(format!(
            "{tool} requires string '{field}'. Try: {}",
            tool_call_example(tool)
        )),
        None => Ok(String::new()),
    }
}

fn optional_bool(args: &Map<String, Value>, field: &str, tool: &str) -> Result<bool, String> {
    match args.get(field) {
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!(
            "{tool} requires boolean '{field}'. Try: {}",
            tool_call_example(tool)
        )),
        None => Ok(false),
    }
}

fn optional_string_vec(
    args: &Map<String, Value>,
    field: &str,
    tool: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = args.get(field) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(format!(
            "{tool} requires '{field}' to be an array of strings. Try: {}",
            tool_call_example(tool)
        ));
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(ToString::to_string).ok_or_else(|| {
                format!(
                    "{tool} requires '{field}' to be an array of strings. Try: {}",
                    tool_call_example(tool)
                )
            })
        })
        .collect()
}

fn command_outcome_json(records: usize, output: Option<&str>) -> String {
    let output = output.map(json_str).unwrap_or_else(|| "null".to_string());
    format!(r#"{{"records":{records},"output":{output}}}"#)
}

fn command_dry_run_json(records: usize) -> String {
    format!(r#"{{"dryRun":true,"records":{records}}}"#)
}

fn query_value_json(capability: &str, query: &str, value: QueryValue) -> String {
    let value = match value {
        QueryValue::Bool(value) => value.to_string(),
        QueryValue::U64(Some(value)) => value.to_string(),
        QueryValue::U64(None) => "null".to_string(),
    };
    format!(
        r#"{{"capability":{},"query":{},"value":{value}}}"#,
        json_str(capability),
        json_str(query)
    )
}

fn workflows_list_json() -> String {
    json!({
        "workflows": [
            {
                "name": "make_js_kv_app",
                "summary": "Create, validate, register, inspect, and invoke a small JS app that uses ctx.resource.kv.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "make_js_kv_app"}}
            },
            {
                "name": "register_app_bundle",
                "summary": "Validate an existing app bundle directory and register it safely through app.add.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "register_app_bundle"}}
            },
            {
                "name": "inspect_app_actions",
                "summary": "List apps and discover one app's self-declared verbs before invoking it.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "inspect_app_actions"}}
            },
            {
                "name": "run_app_action",
                "summary": "Invoke one discovered app verb and read its output.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "run_app_action"}}
            },
            {
                "name": "safe_capability_command",
                "summary": "Use command help and dry-run before dispatching a capability command.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "safe_capability_command"}}
            }
        ],
        "notes": [
            "Always call MCP tools through JSON-RPC method tools/call.",
            "Prefer app_register for app bundle registration; it validates the bundle and still dispatches app.add through core.",
            "JSON results include structuredContent and a text JSON copy for compatibility."
        ]
    })
    .to_string()
}

fn workflow_info_json(name: &str) -> Result<String, String> {
    let workflow = match name.trim() {
        "make_js_kv_app" => json!({
            "name": "make_js_kv_app",
            "goal": "Build and run a small JS app backed by kv.",
            "steps": [
                {"tool": "app_recipe", "arguments": {"kind": "js_kv_app"}, "why": "Read the happy path before doing work."},
                {"tool": "app_scaffold", "arguments": {"id": "notes-demo", "name": "Notes Demo"}, "why": "Get a valid manifest.json and main.js template as JSON files."},
                {"action": "write_files", "why": "Create a bundle directory and write each returned file path/content there."},
                {"tool": "app_bundle_validate", "arguments": {"path": "/path/to/bundle"}, "why": "Catch missing manifest/backend/UI files before registration."},
                {"tool": "app_register", "arguments": {"source": "/path/to/bundle", "dryRun": true}, "why": "Validate app.add through core without committing."},
                {"tool": "app_register", "arguments": {"source": "/path/to/bundle"}, "why": "Commit the app.add event through core."},
                {"tool": "list_apps", "arguments": {}, "why": "Confirm the app id appears."},
                {"tool": "app_actions", "arguments": {"app": "notes-demo"}, "why": "Discover verbs before invoking."},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "write", "args": ["hello"]}, "why": "Run one action."},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "read", "args": []}, "why": "Read back app state."}
            ]
        }),
        "register_app_bundle" => json!({
            "name": "register_app_bundle",
            "goal": "Register an app bundle path safely.",
            "steps": [
                {"tool": "app_bundle_validate", "arguments": {"path": "/path/to/bundle"}},
                {"tool": "app_register", "arguments": {"source": "/path/to/bundle", "dryRun": true}},
                {"tool": "app_register", "arguments": {"source": "/path/to/bundle"}},
                {"tool": "list_apps", "arguments": {}}
            ],
            "fallback": "If app_register is unavailable, call capability_command with {\"name\":\"app.add\",\"help\":true}, then dryRun, then commit."
        }),
        "inspect_app_actions" => json!({
            "name": "inspect_app_actions",
            "goal": "Find an app and learn its verbs.",
            "steps": [
                {"tool": "list_apps", "arguments": {}},
                {"tool": "app_actions", "arguments": {"app": "APP_ID_FROM_LIST"}}
            ]
        }),
        "run_app_action" => json!({
            "name": "run_app_action",
            "goal": "Run one verb after app_actions documents it.",
            "steps": [
                {"tool": "app_actions", "arguments": {"app": "APP_ID"}},
                {"tool": "invoke", "arguments": {"app": "APP_ID", "verb": "VERB_FROM_ACTIONS", "args": []}}
            ]
        }),
        "safe_capability_command" => json!({
            "name": "safe_capability_command",
            "goal": "Use a low-level capability command safely.",
            "steps": [
                {"tool": "capability_command", "arguments": {"name": "app.add", "help": true}, "why": "Get ordered argv params, errors, emits, effects, and examples."},
                {"tool": "capability_command", "arguments": {"name": "app.add", "args": ["demo", "Demo"], "dryRun": true}, "why": "Validate without committing when supported."},
                {"tool": "capability_command", "arguments": {"name": "app.add", "args": ["demo", "Demo"]}, "why": "Commit only after help and dry-run."}
            ],
            "warnings": [
                "Never call effect/runtime commands with dryRun:false unless the workflow requires it.",
                "Prefer app_register for bundle registration because it validates files before app.add."
            ]
        }),
        other => {
            return Err(format!(
                "unknown workflow: {other}. Try: {}",
                tool_call_example(TOOL_WORKFLOWS_LIST)
            ))
        }
    };
    Ok(workflow.to_string())
}

fn app_recipe_json(kind: &str) -> String {
    let kind = defaulted(kind, "js_kv_app");
    json!({
        "kind": kind,
        "summary": "Happy path for building a small JS Terrane app.",
        "steps": [
            "Call app_scaffold with id/name to get manifest.json and main.js files.",
            "Write the returned files into a bundle directory.",
            "Call app_bundle_validate with that directory.",
            "Call app_register with dryRun:true.",
            "Call app_register without dryRun to commit through core app.add.",
            "Call app_actions, then invoke only documented verbs."
        ],
        "requiredFiles": ["manifest.json", "main.js"],
        "defaultManifest": {
            "runtime": "js",
            "backend": "main.js",
            "resources": ["kv"]
        },
        "resourcePattern": "Use ctx.resource.kv.get/set/rm/all/scan/range/keys inside handle(input).",
        "firstCalls": [
            {"tool": "app_scaffold", "arguments": {"id": "notes-demo", "name": "Notes Demo"}},
            {"tool": "app_bundle_validate", "arguments": {"path": "/path/to/bundle"}},
            {"tool": "app_register", "arguments": {"source": "/path/to/bundle", "dryRun": true}}
        ]
    })
    .to_string()
}

fn app_scaffold_json(
    app_id: &str,
    name: &str,
    kind: &str,
    with_ui: bool,
) -> Result<String, String> {
    validate_safe_id(app_id)
        .map_err(|e| format!("{e}. Try: {}", tool_call_example(TOOL_APP_SCAFFOLD)))?;
    let kind = defaulted(kind, "js_kv_notes");
    if kind != "js_kv_notes" && kind != "js_kv_app" {
        return Err(format!(
            "unknown scaffold kind: {kind}. Try kind \"js_kv_notes\" or omit kind."
        ));
    }
    let app_id_js = serde_json::to_string(app_id).unwrap_or_else(|_| "\"app\"".to_string());
    let name_js = serde_json::to_string(name).unwrap_or_else(|_| "\"App\"".to_string());
    let manifest = if with_ui {
        json!({
            "id": app_id,
            "name": name,
            "runtime": "js",
            "backend": "main.js",
            "ui": "index.html",
            "resources": ["kv"]
        })
    } else {
        json!({
            "id": app_id,
            "name": name,
            "runtime": "js",
            "backend": "main.js",
            "resources": ["kv"]
        })
    };
    let main_js = format!(
        r#"function handle(input) {{
  var verb = input[0] || "";
  var kv = ctx.resource.kv;

  if (verb === "__actions__") {{
    return JSON.stringify({{
      app: {app_id_js},
      title: {name_js},
      description: "Generated JS kv notes app.",
      actions: [
        {{ verb: "write", summary: "Store a note.", args: [{{ name: "text", required: true, summary: "note text" }}], returns: "stored note text" }},
        {{ verb: "read", summary: "Read the current note.", args: [], returns: "note text or (empty)" }},
        {{ verb: "clear", summary: "Delete the note.", args: [], returns: "cleared" }}
      ]
    }});
  }}

  if (verb === "write") {{
    var text = input.slice(1).join(" ");
    kv.set("note", text);
    return "stored: " + text;
  }}

  if (verb === "read") {{
    var note = kv.get("note");
    return note == null ? "(empty)" : note;
  }}

  if (verb === "clear") {{
    kv.rm("note");
    return "cleared";
  }}

  return "unknown verb: " + verb;
}}
"#
    );
    let mut files = vec![
        json!({"path": "manifest.json", "content": manifest.to_string()}),
        json!({"path": "main.js", "content": main_js}),
    ];
    if with_ui {
        files.push(json!({
            "path": "index.html",
            "content": "<!doctype html><html><head><title>Terrane App</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><main><h1>Terrane App</h1><button id=\"read\">Read</button><pre id=\"out\"></pre><script>document.getElementById('read').onclick=async()=>{document.getElementById('out').textContent=await window.terrane.invoke('read')};</script></main></body></html>"
        }));
        files.push(json!({
            "path": "style.css",
            "content": "body { font-family: system-ui, sans-serif; margin: 24px; } button { margin: 8px 0; }"
        }));
    }
    Ok(json!({
        "kind": kind,
        "files": files,
        "next": [
            "Write each file path/content into a bundle directory.",
            "Call app_bundle_validate with that directory.",
            "Call app_register with dryRun:true, then app_register without dryRun."
        ]
    })
    .to_string())
}

#[derive(Clone)]
struct BundleInfo {
    id: String,
    name: String,
    runtime: String,
    backend: String,
    ui: String,
    resources: Vec<String>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn inspect_app_bundle(path: &str) -> Result<BundleInfo, String> {
    let bundle = std::path::Path::new(path);
    let manifest = crate::read_manifest(bundle).map_err(|e| {
        format!(
            "app_bundle_validate could not read manifest.json: {e}. Try: {}",
            tool_call_example(TOOL_APP_SCAFFOLD)
        )
    })?;
    let id = manifest.id.trim().to_string();
    let name = defaulted(&manifest.name, &id).to_string();
    let runtime = defaulted(&manifest.runtime, "js").to_string();
    let backend = manifest.backend.trim().to_string();
    let ui = manifest.ui.trim().to_string();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Err(e) = validate_safe_id(&id) {
        errors.push(e);
    }
    if !matches!(runtime.as_str(), "js" | "wasm") {
        errors.push(format!(
            "manifest.runtime {runtime:?} is unsupported; use \"js\" or \"wasm\""
        ));
    }
    if runtime == "js" {
        if backend.is_empty() {
            errors.push("manifest.backend is required for js apps".to_string());
        } else {
            validate_bundle_ref(bundle, "manifest.backend", &backend, &mut errors);
        }
    }
    if !ui.is_empty() {
        validate_bundle_ref(bundle, "manifest.ui", &ui, &mut errors);
    } else {
        warnings.push("manifest.ui is omitted; app is backend-only over invoke/MCP".to_string());
    }
    if manifest.resources.is_empty() {
        warnings.push(
            "manifest.resources is empty; add resources such as \"kv\" when needed".to_string(),
        );
    }

    Ok(BundleInfo {
        id,
        name,
        runtime,
        backend,
        ui,
        resources: manifest.resources,
        errors,
        warnings,
    })
}

fn app_bundle_validate_json(path: &str) -> Result<String, String> {
    let info = inspect_app_bundle(path)?;
    Ok(bundle_info_json(path, &info).to_string())
}

fn app_register_json(
    core: &mut HostCore,
    source: &str,
    id_override: &str,
    name_override: &str,
    runtime_override: &str,
    dry_run: bool,
) -> Result<String, String> {
    let info = inspect_app_bundle(source)?;
    if !info.errors.is_empty() {
        return Err(format!(
            "app_register refused invalid bundle: {}. Call app_bundle_validate for structured details.",
            info.errors.join("; ")
        ));
    }
    let app_id = defaulted(id_override, &info.id).to_string();
    let name = defaulted(name_override, &info.name).to_string();
    let runtime = defaulted(runtime_override, &info.runtime).to_string();
    validate_safe_id(&app_id)?;
    if !matches!(runtime.as_str(), "js" | "wasm") {
        return Err(format!(
            "app_register requires runtime \"js\" or \"wasm\", got {runtime:?}"
        ));
    }
    let argv = vec![
        app_id.clone(),
        name.clone(),
        "--source".to_string(),
        source.to_string(),
        "--runtime".to_string(),
        runtime.clone(),
    ];
    if dry_run {
        let outcome = crate::dry_run_on_core(core, "app.add", &argv)?;
        Ok(json!({
            "dryRun": true,
            "command": "app.add",
            "args": argv,
            "records": outcome.records,
            "app": {"id": app_id, "name": name, "runtime": runtime, "source": source},
            "next": "Call app_register again without dryRun to commit."
        })
        .to_string())
    } else {
        let outcome = crate::dispatch_on_core(core, "app.add", &argv)?;
        Ok(json!({
            "command": "app.add",
            "args": argv,
            "records": outcome.records.len(),
            "output": outcome.output,
            "app": {"id": app_id, "name": name, "runtime": runtime, "source": source},
            "next": [
                {"tool": "list_apps", "arguments": {}},
                {"tool": "app_actions", "arguments": {"app": app_id}}
            ]
        })
        .to_string())
    }
}

fn bundle_info_json(path: &str, info: &BundleInfo) -> Value {
    json!({
        "path": path,
        "valid": info.errors.is_empty(),
        "app": {
            "id": info.id,
            "name": info.name,
            "runtime": info.runtime,
            "backend": info.backend,
            "ui": info.ui,
            "resources": info.resources
        },
        "errors": info.errors,
        "warnings": info.warnings,
        "next": if info.errors.is_empty() {
            json!([
                {"tool": "app_register", "arguments": {"source": path, "dryRun": true}},
                {"tool": "app_register", "arguments": {"source": path}}
            ])
        } else {
            json!("Fix errors, then call app_bundle_validate again.")
        }
    })
}

fn validate_bundle_ref(
    bundle: &std::path::Path,
    label: &str,
    reference: &str,
    errors: &mut Vec<String>,
) {
    if !is_safe_relative_path(reference) {
        errors.push(format!(
            "{label} must be a safe relative path inside the bundle, got {reference:?}"
        ));
        return;
    }
    if !bundle.join(reference).is_file() {
        errors.push(format!("{label} references missing file {reference:?}"));
    }
}

fn is_safe_relative_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    !path.as_os_str().is_empty()
        && path.is_relative()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn validate_safe_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("app id must not be empty".to_string());
    }
    if id == "." || id == ".." {
        return Err(format!("app id is unsafe: {id:?}"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!(
            "app id {id:?} is unsafe; use ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn defaulted<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() {
        fallback
    } else {
        value
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

fn tool_json(id: &str, json_text: &str, is_error: bool) -> String {
    match serde_json::from_str::<Value>(json_text) {
        Ok(value) => tool_value(id, json_text, &value, is_error),
        Err(_) => tool_text(id, json_text, is_error),
    }
}

fn tool_json_if_possible(id: &str, text: &str, is_error: bool) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => tool_value(id, text, &value, is_error),
        Err(_) => tool_text(id, text, is_error),
    }
}

fn tool_value(id: &str, text: &str, value: &Value, is_error: bool) -> String {
    let result = format!(
        r#"{{"content":[{{"type":"text","text":{}}}],"structuredContent":{},"isError":{is_error}}}"#,
        json_str(text),
        value
    );
    ok(id, &result)
}

fn tool_call_example(tool: &str) -> String {
    let arguments = match tool {
        TOOL_WORKFLOW_INFO => json!({"name": "make_js_kv_app"}),
        TOOL_APP_RECIPE => json!({"kind": "js_kv_app"}),
        TOOL_APP_SCAFFOLD => json!({"id": "notes-demo", "name": "Notes Demo"}),
        TOOL_APP_BUNDLE_VALIDATE => json!({"path": "/path/to/bundle"}),
        TOOL_APP_REGISTER => json!({"source": "/path/to/bundle", "dryRun": true}),
        TOOL_APP_ACTIONS => json!({"app": "APP_ID"}),
        TOOL_INVOKE => json!({"app": "APP_ID", "verb": "read", "args": []}),
        TOOL_CAPABILITIES_LIST => json!({}),
        TOOL_CAPABILITY_INFO => json!({"namespace": "app", "format": "json"}),
        TOOL_CAPABILITY_QUERY => {
            json!({"capability": "app", "query": "exists", "args": ["APP_ID"]})
        }
        TOOL_CAPABILITY_COMMAND => json!({"name": "app.add", "help": true}),
        TOOL_LIST_APPS | TOOL_WORKFLOWS_LIST => json!({}),
        _ => json!({}),
    };
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments
        }
    })
    .to_string()
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
#[path = "mcp_tests.rs"]
mod tests;
