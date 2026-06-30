//! Shared MCP semantics for Terrane host transports.
//!
//! Stdio and HTTP are just transports. This module owns the JSON-RPC request
//! handling and MCP tool behavior so every host exposes the same list ->
//! discover -> act contract.

use std::collections::BTreeSet;

use nanoserde::{DeJson, SerJson};
use serde_json::{json, Map, Value};
use terrane_api::{
    mcp_prompts, mcp_resource_templates, mcp_resources, mcp_tools, MCP_PROTOCOL_VERSION,
    TOOL_APP_ACTIONS, TOOL_APP_BUNDLE_VALIDATE, TOOL_APP_RECIPE, TOOL_APP_REGISTER,
    TOOL_APP_REGISTER_INLINE, TOOL_APP_SCAFFOLD, TOOL_CAPABILITIES_LIST, TOOL_CAPABILITY_COMMAND,
    TOOL_CAPABILITY_INFO, TOOL_CAPABILITY_QUERY, TOOL_INVOKE, TOOL_LIST_APPS, TOOL_WORKFLOWS_LIST,
    TOOL_WORKFLOW_INFO,
};
use terrane_core::QueryValue;

use crate::{BundleManifest, HostCore};

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
        Some("resources/list") => id.map(|id| ok(id, &resources_list_result())),
        Some("resources/templates/list") => id.map(|id| ok(id, &resource_templates_list_result())),
        Some("resources/read") => {
            id.map(|id| resource_read(core, id, field("params").unwrap_or("{}")))
        }
        Some("prompts/list") => id.map(|id| ok(id, &prompts_list_result())),
        Some("prompts/get") => id.map(|id| prompt_get(id, field("params").unwrap_or("{}"))),
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
        r#"{{"protocolVersion":{},"capabilities":{{"tools":{{"listChanged":false}},"resources":{{"subscribe":false,"listChanged":false}},"prompts":{{"listChanged":false}}}},"serverInfo":{{"name":"terrane-mcp","version":"0.1.0"}}}}"#,
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

fn resources_list_result() -> String {
    let resources: Vec<String> = mcp_resources()
        .iter()
        .map(|resource| {
            format!(
                r#"{{"uri":{},"name":{},"description":{},"mimeType":{}}}"#,
                json_str(resource.uri),
                json_str(resource.name),
                json_str(resource.description),
                json_str(resource.mime_type)
            )
        })
        .collect();
    format!(r#"{{"resources":[{}]}}"#, resources.join(","))
}

fn resource_templates_list_result() -> String {
    let templates: Vec<String> = mcp_resource_templates()
        .iter()
        .map(|template| {
            format!(
                r#"{{"uriTemplate":{},"name":{},"description":{},"mimeType":{}}}"#,
                json_str(template.uri_template),
                json_str(template.name),
                json_str(template.description),
                json_str(template.mime_type)
            )
        })
        .collect();
    format!(r#"{{"resourceTemplates":[{}]}}"#, templates.join(","))
}

fn prompts_list_result() -> String {
    let prompts: Vec<String> = mcp_prompts()
        .iter()
        .map(|prompt| {
            let arguments = prompt_arguments(prompt.arguments_schema).unwrap_or_default();
            format!(
                r#"{{"name":{},"description":{},"arguments":{}}}"#,
                json_str(prompt.name),
                json_str(prompt.description),
                arguments
            )
        })
        .collect();
    format!(r#"{{"prompts":[{}]}}"#, prompts.join(","))
}

const MCP_DOC_INDEX: &str = include_str!("../../../../host/mcp/docs/README.md");
const MCP_DOC_CLIENTS: &str = include_str!("../../../../host/mcp/docs/CLIENTS.md");
const MCP_DOC_APP_BUILDING: &str = include_str!("../../../../host/mcp/docs/APP_BUILDING.md");
const MCP_DOC_CAPABILITY_OPERATIONS: &str =
    include_str!("../../../../host/mcp/docs/CAPABILITY_OPERATIONS.md");
const MCP_DOC_SECURITY: &str = include_str!("../../../../host/mcp/docs/SECURITY.md");
const MCP_DOC_WEAK_MODELS: &str = include_str!("../../../../host/mcp/docs/WEAK_MODELS.md");

fn resource_content(core: &mut HostCore, uri: &str) -> Result<(&'static str, String), String> {
    match uri {
        "terrane://docs/index" => Ok(("text/markdown", MCP_DOC_INDEX.to_string())),
        "terrane://docs/clients" => Ok(("text/markdown", MCP_DOC_CLIENTS.to_string())),
        "terrane://docs/app-building" => Ok(("text/markdown", MCP_DOC_APP_BUILDING.to_string())),
        "terrane://docs/capability-operations" => {
            Ok(("text/markdown", MCP_DOC_CAPABILITY_OPERATIONS.to_string()))
        }
        "terrane://docs/security" => Ok(("text/markdown", MCP_DOC_SECURITY.to_string())),
        "terrane://docs/weak-models" => Ok(("text/markdown", MCP_DOC_WEAK_MODELS.to_string())),
        _ => {
            if let Some(namespace) = uri.strip_prefix("terrane://capabilities/") {
                if namespace.trim().is_empty() || namespace.contains('/') {
                    return Err(format!("invalid capability resource uri: {uri}"));
                }
                let doc = crate::cap_doc::render_capability_info(namespace, "markdown", false)?;
                return Ok(("text/markdown", doc));
            }
            if let Some(name) = uri.strip_prefix("terrane://workflows/") {
                if name.trim().is_empty() || name.contains('/') {
                    return Err(format!("invalid workflow resource uri: {uri}"));
                }
                let workflow = workflow_info_json(name)?;
                return Ok(("application/json", workflow));
            }
            if let Some(app) = uri.strip_prefix("terrane://apps/") {
                let Some(app) = app.strip_suffix("/actions") else {
                    return Err(format!("unknown resource uri: {uri}"));
                };
                if app.trim().is_empty() || app.contains('/') {
                    return Err(format!("invalid app actions resource uri: {uri}"));
                }
                let actions = crate::app_actions(core, app)?;
                return Ok(("application/json", actions));
            }
            Err(format!("unknown resource uri: {uri}"))
        }
    }
}

fn prompt_arguments(schema: &str) -> Option<Value> {
    let schema = serde_json::from_str::<Value>(schema).ok()?;
    let properties = schema.get("properties")?.as_object()?;
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let args: Vec<Value> = properties
        .iter()
        .map(|(name, spec)| {
            json!({
                "name": name,
                "description": spec.get("description").and_then(Value::as_str).unwrap_or(""),
                "required": required.contains(name.as_str())
            })
        })
        .collect();
    Some(Value::Array(args))
}

fn prompt_content(
    name: &str,
    arguments: &Map<String, Value>,
) -> Result<(&'static str, String), String> {
    match name {
        "make_js_kv_app" => {
            let app_id = prompt_string(arguments, "id", "notes-demo");
            let app_name = prompt_string(arguments, "name", "Notes Demo");
            let text = prompt_string(arguments, "text", "hello from Terrane MCP");
            Ok((
                "Create and run a JS kv app through Terrane MCP.",
                format!(
                    r#"Create and verify a Terrane JS kv notes app without reading repository source files.

Use MCP tools only:
1. Call `app_recipe` with {{"kind":"js_kv_app"}}.
2. Call `app_scaffold` with {{"id":{app_id_json},"name":{app_name_json}}}.
3. Take the returned `structuredContent.files` array and call `app_register_inline` with {{"files": <that array>, "dryRun": true}}. Do not JSON-stringify the files array.
4. If dry-run succeeds, call `app_register_inline` again with the same complete `files` array and no `dryRun`.
5. Call `list_apps`, then `app_actions` for {app_id_json}.
6. Invoke `write` with {text_json}, invoke `read`, invoke `clear`, then invoke `read` again.

Do not use shell, source-file reads, glob, grep, or broad filesystem listing. If you need capability semantics, read `terrane://capabilities/kv` or call `capability_info` for `kv`.
"#,
                    app_id_json = json_str(&app_id),
                    app_name_json = json_str(&app_name),
                    text_json = json_str(&text),
                ),
            ))
        }
        "register_app_bundle" => {
            let source = prompt_string(arguments, "source", "/path/to/bundle");
            Ok((
                "Validate and register an existing Terrane app bundle.",
                format!(
                    r#"Register an existing Terrane app bundle through MCP.

Use this exact route:
1. Call `app_bundle_validate` with {{"path":{source_json}}}.
2. If `structuredContent.valid` is true, call `app_register` with {{"source":{source_json},"dryRun":true}}.
3. If dry-run succeeds, call `app_register` with {{"source":{source_json}}}.
4. Confirm with `list_apps`, then inspect with `app_actions`.

Do not fall back to `capability_command app.add` unless `app_register` is unavailable.
"#,
                    source_json = json_str(&source)
                ),
            ))
        }
        "inspect_app_actions" => {
            let app = prompt_string(arguments, "app", "APP_ID");
            Ok((
                "List apps and inspect one app's actions.",
                format!(
                    r#"Discover an app before acting.

1. Call `list_apps`.
2. Choose the target id, or use {app_json} if it exists.
3. Call `app_actions` with {{"app": <id>}}.
4. Only call `invoke` with verbs and args documented by `app_actions`.
"#,
                    app_json = json_str(&app)
                ),
            ))
        }
        "safe_capability_command" => {
            let command = prompt_string(arguments, "command", "app.add");
            Ok((
                "Use a low-level Terrane capability command safely.",
                format!(
                    r#"Operate a low-level capability command through MCP.

1. Call `capability_command` with {{"name":{command_json},"help":true}}.
2. Read ordered params, returns, errors, emitted events, and effects.
3. Prefer a purpose-built app tool if one exists, such as `app_register` or `app_register_inline`.
4. If this command supports dry-run, call `capability_command` with `dryRun:true`.
5. Commit only after the help and dry-run results are explicit.

For capability-specific details, read `terrane://capabilities/<namespace>` or call `capability_info`.
"#,
                    command_json = json_str(&command)
                ),
            ))
        }
        other => Err(format!("unknown prompt: {other}")),
    }
}

fn prompt_string(args: &Map<String, Value>, name: &str, fallback: &str) -> String {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn is_tool_name(name: &str) -> bool {
    mcp_tools().iter().any(|tool| tool.name == name)
}

struct ReadResourceParams {
    uri: String,
}

struct GetPromptParams {
    name: String,
    arguments: Map<String, Value>,
}

struct CallParams {
    name: String,
    arguments: Value,
}

fn resource_read(core: &mut HostCore, id: &str, params_raw: &str) -> String {
    let params = match parse_resource_read_params(params_raw) {
        Ok(params) => params,
        Err(e) => return error(id, -32602, &e),
    };
    match resource_content(core, &params.uri) {
        Ok((mime_type, text)) => ok(
            id,
            &json!({
                "contents": [
                    {
                        "uri": params.uri,
                        "mimeType": mime_type,
                        "text": text
                    }
                ]
            })
            .to_string(),
        ),
        Err(e) => error(id, -32602, &e),
    }
}

fn prompt_get(id: &str, params_raw: &str) -> String {
    let params = match parse_prompt_get_params(params_raw) {
        Ok(params) => params,
        Err(e) => return error(id, -32602, &e),
    };
    match prompt_content(&params.name, &params.arguments) {
        Ok((description, text)) => ok(
            id,
            &json!({
                "description": description,
                "messages": [
                    {
                        "role": "user",
                        "content": {"type": "text", "text": text}
                    }
                ]
            })
            .to_string(),
        ),
        Err(e) => error(id, -32602, &e),
    }
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
        TOOL_APP_REGISTER_INLINE => {
            let args = match args_object(TOOL_APP_REGISTER_INLINE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let id_override = match optional_string(args, "id", TOOL_APP_REGISTER_INLINE) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let name_override = match optional_string(args, "name", TOOL_APP_REGISTER_INLINE) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let runtime_override = match optional_string(args, "runtime", TOOL_APP_REGISTER_INLINE)
            {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let files = match required_inline_files(args, "files", TOOL_APP_REGISTER_INLINE) {
                Ok(files) => files,
                Err(e) => return tool_text(id, &e, true),
            };
            let dry_run = match optional_bool(args, "dryRun", TOOL_APP_REGISTER_INLINE) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_register_inline_json(
                core,
                &id_override,
                &name_override,
                &runtime_override,
                files,
                dry_run,
            ) {
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
            match crate::app_actions_checked(core, &app) {
                Ok(output) => tool_json_if_possible(id, &output, false),
                Err(crate::InvokeFailure::PermissionRequired(required)) => {
                    tool_json(id, &required.serialize_json(), true)
                }
                Err(crate::InvokeFailure::Other(e)) => tool_text(id, &e, true),
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
            match crate::invoke_app_checked(core, &app, &verb, &argv) {
                Ok(output) => tool_text(id, &output, false),
                Err(crate::InvokeFailure::PermissionRequired(required)) => {
                    tool_json(id, &required.serialize_json(), true)
                }
                Err(crate::InvokeFailure::Other(e)) => tool_text(id, &e, true),
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

fn parse_resource_read_params(params_raw: &str) -> Result<ReadResourceParams, String> {
    let value: Value =
        serde_json::from_str(params_raw).map_err(|e| format!("invalid params: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "invalid params: resources/read params must be an object".to_string())?;
    let uri = object
        .get("uri")
        .and_then(Value::as_str)
        .filter(|uri| !uri.trim().is_empty())
        .ok_or_else(|| {
            "invalid params: resources/read requires non-empty string 'uri'".to_string()
        })?
        .to_string();
    Ok(ReadResourceParams { uri })
}

fn parse_prompt_get_params(params_raw: &str) -> Result<GetPromptParams, String> {
    let value: Value =
        serde_json::from_str(params_raw).map_err(|e| format!("invalid params: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "invalid params: prompts/get params must be an object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "invalid params: prompts/get requires non-empty string 'name'".to_string())?
        .to_string();
    let arguments = match object.get("arguments") {
        Some(Value::Object(args)) => args.clone(),
        Some(_) => {
            return Err("invalid params: prompts/get 'arguments' must be an object".to_string())
        }
        None => Map::new(),
    };
    Ok(GetPromptParams { name, arguments })
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

#[derive(Debug, Clone)]
struct InlineFile {
    path: String,
    content: String,
}

fn required_inline_files(
    args: &Map<String, Value>,
    field: &str,
    tool: &str,
) -> Result<Vec<InlineFile>, String> {
    let Some(value) = args.get(field) else {
        return Err(format!(
            "{tool} requires '{field}'. Try: {}",
            tool_call_example(tool)
        ));
    };
    let Some(items) = value.as_array() else {
        let hint = if value.is_string() {
            " Received a string; pass structuredContent.files as a JSON array and do not JSON-stringify it."
        } else {
            " Pass structuredContent.files directly from app_scaffold."
        };
        return Err(format!(
            "{tool} requires '{field}' to be an array of {{path, content}} objects.{hint} Try: {}",
            tool_call_example(tool)
        ));
    };
    let mut files = Vec::new();
    for item in items {
        let Some(file) = item.as_object() else {
            return Err(format!(
                "{tool} requires every file to be an object. Try: {}",
                tool_call_example(tool)
            ));
        };
        let path = file.get("path").and_then(Value::as_str).ok_or_else(|| {
            format!(
                "{tool} requires every file to have string path. Try: {}",
                tool_call_example(tool)
            )
        })?;
        let content = file.get("content").and_then(Value::as_str).ok_or_else(|| {
            format!(
                "{tool} requires every file to have string content. Try: {}",
                tool_call_example(tool)
            )
        })?;
        if !is_safe_relative_path(path) {
            return Err(format!(
                "{tool} file path must be a safe relative bundle path, got {path:?}"
            ));
        }
        files.push(InlineFile {
            path: path.to_string(),
            content: content.to_string(),
        });
    }
    if files.is_empty() {
        return Err(format!(
            "{tool} requires at least one file. Try: {}",
            tool_call_example(tool)
        ));
    }
    Ok(files)
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
                "name": "make_js_kv_app_no_filesystem",
                "summary": "Create and register a JS kv app using only MCP structured data, without shell/source/list access.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "make_js_kv_app_no_filesystem"}}
            },
            {
                "name": "make_js_multicap_app_no_filesystem",
                "summary": "Create and verify a JS app using app, kv, crdt, relational_db, and replica with no shell/source/list access.",
                "firstCall": {"tool": "workflow_info", "arguments": {"name": "make_js_multicap_app_no_filesystem"}}
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
        "chooseByOutcome": [
            {"when": "simple notes, one app, key/value storage only", "workflow": "make_js_kv_app_no_filesystem"},
            {"when": "locked-down client with no filesystem tools must create an app", "workflow": "make_js_kv_app_no_filesystem"},
            {"when": "visible calendar, dashboard, form, natural-language input page, or other UI app backed by app state", "workflow": "make_js_kv_app_no_filesystem", "use": "app_scaffold with withUi:true; keep browser behavior in ui.js for non-trivial pages"},
            {"when": "task asks for five surfaces, multiple capabilities, relational data, CRDT/collaboration, or replica identity", "workflow": "make_js_multicap_app_no_filesystem"},
            {"when": "bundle directory already exists", "workflow": "register_app_bundle"},
            {"when": "app already exists and the task is to operate it", "workflow": "inspect_app_actions"}
        ],
        "notes": [
            "Always call MCP tools through JSON-RPC method tools/call.",
            "Choose a workflow by matching the user's desired outcome to chooseByOutcome before acting.",
            "For locked-down clients, prefer app_register_inline after app_scaffold; it writes the owned bundle under TERRANE_HOME/apps/<id> on commit.",
            "app_register_inline.files must be the structuredContent.files JSON array, not a JSON string.",
            "Every app_register_inline retry must include the complete files array: manifest.json, backend, manifest.ui, ui.js/style.css, and assets.",
            "After app_scaffold, the next assistant action should be app_register_inline with dryRun:true. Do not print the whole app as prose/code before the dry run.",
            "For UI apps, window.terrane.invoke takes positional string args: invoke(\"verb\", \"arg1\", \"arg2\"), not invoke(\"verb\", [\"arg1\", \"arg2\"]).",
            "For optional KV indexes such as event_ids, use a kvGetOrNull helper and default missing keys to [] before JSON.parse.",
            "When the user asked for an interactive page, verify the page itself when possible; backend invoke success alone does not prove the UI works.",
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
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array. Do not emit the whole app as prose/code before the dry run."
            },
            "steps": [
                {"tool": "app_recipe", "arguments": {"kind": "js_kv_app"}, "why": "Read the happy path before doing work."},
                {"tool": "app_scaffold", "arguments": {"id": "notes-demo", "name": "Notes Demo"}, "why": "Get a valid manifest.json and main.js template as JSON files. Add withUi:true when the requested outcome is a visible app."},
                {"tool": "app_register_inline", "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true}, "why": "Validate app.add through core without filesystem tools or committing."},
                {"tool": "app_register_inline", "arguments": {"files": "same files from app_scaffold"}, "why": "Write the owned bundle under TERRANE_HOME/apps/<id> and commit app.add through core."},
                {"tool": "list_apps", "arguments": {}, "why": "Confirm the app id appears."},
                {"tool": "app_actions", "arguments": {"app": "notes-demo"}, "why": "Discover verbs before invoking."},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "write", "args": ["hello"]}, "why": "Run one action."},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "read", "args": []}, "why": "Read back app state."}
            ],
            "uiContract": {
                "when": "Use withUi:true for calendars, dashboards, forms, natural-language input boxes, and any requested page.",
                "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. Do not pass [arg1,arg2] unless the backend expects one JSON/string arg.",
                "files": "For non-trivial UI, keep index.html small and put browser behavior in ui.js; keep backend handle(input) in main.js.",
                "kvOptionalReads": "For optional KV indexes such as event_ids, use kvGetOrNull(kv, key) and default null to [] before JSON.parse.",
                "verification": "If a browser is available, open /apps/<id>/, check console errors, click one UI control, and confirm the rendered result. Backend invokes alone do not prove a UI app works."
            },
            "inlineFilesContract": "app_register_inline.files must be a JSON array of {path,content} objects. Pass structuredContent.files directly; do not JSON-stringify it. Every retry must include the complete files array, not only changed files.",
            "replaceExisting": "If app_register_inline says the id already exists and you are replacing a broken generated app, call capability_command {name:\"app.remove\",help:true}, then dryRun, then remove, then register again. If the task is to operate the existing app, use app_actions instead.",
            "pathBundleAlternative": "If the client can write files itself, write app_scaffold files to a bundle directory, call app_bundle_validate, then app_register dryRun and commit."
        }),
        "make_js_kv_app_no_filesystem" => json!({
            "name": "make_js_kv_app_no_filesystem",
            "goal": "Create a JS kv app when read/list/glob/grep/bash are denied.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array. Do not emit the whole app as prose/code before the dry run."
            },
            "steps": [
                {"tool": "app_scaffold", "arguments": {"id": "notes-demo", "name": "Notes Demo", "withUi": true}, "why": "Get manifest.json, main.js, index.html, and style.css as structuredContent.files for visible apps. Omit withUi only for backend-only tasks."},
                {"tool": "app_register_inline", "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true}, "why": "Validate without writing files or committing."},
                {"tool": "app_register_inline", "arguments": {"files": "same files from app_scaffold"}, "why": "Write under TERRANE_HOME/apps/<id> and commit through app.add."},
                {"tool": "app_actions", "arguments": {"app": "notes-demo"}},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "write", "args": ["hello"]}},
                {"tool": "invoke", "arguments": {"app": "notes-demo", "verb": "read", "args": []}}
            ],
            "uiContract": {
                "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. window.terrane.invoke(\"verb\", [arg1,arg2]) sends one arg and is wrong for two-arg verbs.",
                "separateFiles": "For complex UI, include an extra ui.js file and reference it from index.html instead of generating one huge inline script.",
                "kvOptionalReads": "For optional KV indexes such as event_ids, use kvGetOrNull(kv, key) and default null to [] before JSON.parse.",
                "verification": "For UI outcomes, verify the page loads and rendered results match the requested view. If no browser tool exists, say UI was not live-tested and keep the code conservative."
            },
            "inlineFilesContract": "app_register_inline.files must be a JSON array of {path,content} objects. Pass structuredContent.files directly; do not JSON-stringify it. Every retry must include the complete files array, not only changed files.",
            "replaceExisting": "If the id already exists and the current task is to replace your broken generated app, use capability_command app.remove help, dryRun, commit remove, then app_register_inline again. Do not remove an app just to inspect or operate it.",
            "doNotUse": ["source reads", "shell", "glob", "grep", "filesystem list", "capability_command app.add before app_register_inline", "backend-only proof for visible UI tasks"]
        }),
        "make_js_multicap_app_no_filesystem" => json!({
            "name": "make_js_multicap_app_no_filesystem",
            "goal": "Create and verify a backend JS app that uses five capability surfaces without filesystem/source access.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array. Do not emit the whole app as prose/code before the dry run."
            },
            "capabilitiesUsed": [
                {"namespace": "app", "how": "app_register_inline, app_actions, invoke, and capability_query app.exists"},
                {"namespace": "kv", "how": "ctx.resource.kv inside the generated app"},
                {"namespace": "crdt", "how": "ctx.resource.crdt inside the generated app"},
                {"namespace": "relational_db", "how": "ctx.resource.relational_db inside the generated app"},
                {"namespace": "replica", "how": "capability_command replica.init and capability_query replica.peer"}
            ],
            "steps": [
                {"tool": "capability_info", "arguments": {"namespace": "kv", "format": "json"}, "why": "Review app-scoped KV methods and reserved key constraints."},
                {"tool": "capability_info", "arguments": {"namespace": "crdt", "format": "json"}, "why": "Review map/list/text resource methods."},
                {"tool": "capability_info", "arguments": {"namespace": "relational_db", "format": "json"}, "why": "Review table spec and query method shape."},
                {"tool": "app_scaffold", "arguments": {"id": "multicap-demo", "name": "Multi-cap Demo", "kind": "js_multicap_audit"}, "why": "Get manifest.json and main.js using resources kv, crdt, and relational_db."},
                {"tool": "app_register_inline", "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true}, "why": "Validate app.add through core without writing files."},
                {"tool": "app_register_inline", "arguments": {"files": "same files from app_scaffold"}, "why": "Write under TERRANE_HOME/apps/<id> and commit through app.add."},
                {"tool": "capability_command", "arguments": {"name": "replica.init", "help": true}, "why": "Read effects and emitted events before minting identity."},
                {"tool": "capability_command", "arguments": {"name": "replica.init"}, "why": "Ensure the home has a stable replica peer id."},
                {"tool": "capability_query", "arguments": {"capability": "replica", "query": "peer", "args": []}, "why": "Read folded replica identity without appending records."},
                {"tool": "capability_query", "arguments": {"capability": "app", "query": "exists", "args": ["multicap-demo"]}, "why": "Confirm the app catalog contains the new app."},
                {"tool": "app_actions", "arguments": {"app": "multicap-demo"}, "why": "Discover app verbs before invoking."},
                {"tool": "invoke", "arguments": {"app": "multicap-demo", "verb": "seed", "args": ["multicap seed"]}, "why": "Write KV, CRDT, and relational DB state. The returned JSON is useful, but it does not replace the required pre-clear summary read."},
                {"tool": "invoke", "arguments": {"app": "multicap-demo", "verb": "summary", "args": []}, "why": "Explicitly read all three app resources back before clearKv. Do not count seed output as this pre-clear summary."},
                {"tool": "invoke", "arguments": {"app": "multicap-demo", "verb": "clearKv", "args": []}, "why": "Exercise KV deletion without clearing CRDT or relational state."},
                {"tool": "invoke", "arguments": {"app": "multicap-demo", "verb": "summary", "args": []}, "why": "Make the final post-clear read explicit. clearKv also returns a summary, but call summary again when the task asks for a final summary."}
            ],
            "readPolicy": "For evaluation tasks, invoke summary after seed and again after clearKv. Seed and clearKv return JSON summaries, but mutation returns do not replace the explicit pre-clear and post-clear summary reads.",
            "finalReadPolicy": "For evaluation tasks, invoke summary after clearKv even though clearKv returns JSON. The separate post-clear summary proves the final state was readable after the mutation completed.",
            "inlineFilesContract": "app_register_inline.files must be a JSON array of {path,content} objects. Pass structuredContent.files directly; do not JSON-stringify it. Every retry must include the complete files array, not only changed files.",
            "kvOptionalReads": "Generated apps should use kvGetOrNull(kv, key) for optional KV state and default null before JSON.parse or final summary reads.",
            "successSignals": [
                "app.exists returns true",
                "replica.peer returns a u64 value",
                "seed output contains kv.lastNote, crdt.profile.owner, crdt.events, crdt.journal, relational.active, and relational.p1",
                "the separate pre-clear summary after seed contains kv.lastNote, crdt.profile.owner, crdt.events, crdt.journal, relational.active, and relational.p1",
                "the separate post-clear summary has kv.theme null and kv.lastNote null while relational.p1 and crdt.profile.owner still exist"
            ],
            "doNotUse": ["source reads", "shell", "glob", "grep", "filesystem list", "net/model effects", "capability_command app.add before app_register_inline"]
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
    if kind == "js_multicap_audit" {
        return json!({
            "kind": kind,
            "summary": "Happy path for building a backend JS Terrane app that uses kv, crdt, and relational_db, with replica/app checks over MCP.",
            "steps": [
                "Call workflow_info with make_js_multicap_app_no_filesystem for the complete five-capability route.",
                "If the model started from an outcome-only request, choose this recipe after workflows_list maps multi-cap, relational, CRDT, or replica tasks to make_js_multicap_app_no_filesystem.",
                "Call app_scaffold with kind js_multicap_audit to get manifest.json and main.js files.",
                "For MCP-only clients, pass returned files to app_register_inline with dryRun:true.",
                "Call app_register_inline again without dryRun to write under TERRANE_HOME/apps/<id> and commit through core app.add.",
                "Call capability_command help for replica.init, then replica.init, then capability_query replica.peer.",
                "Call capability_query app.exists for the registered app id.",
                "Call app_actions, invoke seed, invoke summary as a separate pre-clear read, invoke clearKv, then invoke summary again as a separate final read."
            ],
            "requiredFiles": ["manifest.json", "main.js"],
            "defaultManifest": {
                "runtime": "js",
                "backend": "main.js",
                "resources": ["kv", "crdt", "relational_db"]
            },
            "resourcePattern": "Use ctx.resource.kv, ctx.resource.crdt, and ctx.resource.relational_db inside handle(input). Use capability_command/query for replica and app catalog checks.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array before explaining or printing code."
            },
            "uiContract": {
                "when": "If the requested outcome includes a visible page, pass withUi:true to app_scaffold and keep index.html plus ui.js separate from main.js.",
                "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. Do not pass [arg1,arg2] for multiple backend args.",
                "verification": "For UI outcomes, page load and rendered results matter in addition to backend invoke checks."
            },
            "firstCalls": [
                {"tool": "workflow_info", "arguments": {"name": "make_js_multicap_app_no_filesystem"}},
                {"tool": "app_scaffold", "arguments": {"id": "multicap-demo", "name": "Multi-cap Demo", "kind": "js_multicap_audit"}},
                {"tool": "app_register_inline", "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true}},
                {"tool": "app_register_inline", "arguments": {"files": "same files from app_scaffold"}}
            ]
        })
        .to_string();
    }
    json!({
        "kind": kind,
        "summary": "Happy path for building a small JS Terrane app.",
        "steps": [
            "Call app_scaffold with id/name to get manifest.json and main.js files. Add withUi:true for calendars, dashboards, forms, natural-language input pages, or any visible app.",
            "For MCP-only clients, pass returned files to app_register_inline with dryRun:true.",
            "Call app_register_inline again without dryRun to write under TERRANE_HOME/apps/<id> and commit through core app.add.",
            "For clients with a bundle directory, app_bundle_validate then app_register is still supported.",
            "Call app_actions, then invoke only documented verbs."
        ],
        "requiredFiles": ["manifest.json", "main.js"],
        "defaultManifest": {
            "runtime": "js",
            "backend": "main.js",
            "resources": ["kv"]
        },
        "resourcePattern": "Use ctx.resource.kv.get/set/rm/all/scan/range/keys inside handle(input). For optional/index keys, copy kvGetOrNull from app_scaffold and default null to [] before JSON.parse.",
        "inlineFilesContract": "app_register_inline.files must be a JSON array of {path,content} objects. Pass structuredContent.files directly; do not JSON-stringify it. Every retry must include the complete files array.",
        "nextAfterScaffold": {
            "tool": "app_register_inline",
            "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
            "instruction": "After app_scaffold, call this immediately with the complete files array before explaining or printing code."
        },
        "uiContract": {
            "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. Do not use window.terrane.invoke(\"verb\", [arg1,arg2]) for two backend args.",
            "files": "For non-trivial UI, add ui.js and keep index.html mostly markup. Syntax errors in one huge inline script can make the page unusable.",
            "verification": "When the requested outcome is a UI app, verify the page loads and one user-visible flow works; backend invoke checks alone are not enough."
        },
        "firstCalls": [
            {"tool": "app_scaffold", "arguments": {"id": "notes-demo", "name": "Notes Demo"}},
            {"tool": "app_register_inline", "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true}},
            {"tool": "app_register_inline", "arguments": {"files": "same files from app_scaffold"}}
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
    if kind == "js_multicap_audit" {
        return app_multicap_scaffold_json(app_id, name, kind, with_ui);
    }
    if kind != "js_kv_notes" && kind != "js_kv_app" {
        return Err(format!(
            "unknown scaffold kind: {kind}. Try kind \"js_kv_notes\", \"js_multicap_audit\", or omit kind."
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
        r#"function kvGetOrNull(kv, key) {{
  try {{
    return kv.get(key);
  }} catch (err) {{
    if (String(err).indexOf("not found") !== -1) {{
      return null;
    }}
    throw err;
  }}
}}

function kvRmIfPresent(kv, key) {{
  try {{
    kv.rm(key);
  }} catch (err) {{
    if (String(err).indexOf("not found") === -1) {{
      throw err;
    }}
  }}
}}

function handle(input) {{
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
    var note = kvGetOrNull(kv, "note");
    return note == null ? "(empty)" : note;
  }}

  if (verb === "clear") {{
    kvRmIfPresent(kv, "note");
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
            "content": "<!doctype html><html><head><title>Terrane App</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><main><h1>Terrane App</h1><button id=\"read\">Read</button><pre id=\"out\"></pre></main><script src=\"ui.js\"></script></body></html>"
        }));
        files.push(json!({
            "path": "ui.js",
            "content": "document.getElementById('read').onclick = async function () {\n  document.getElementById('out').textContent = await window.terrane.invoke('read');\n};\n"
        }));
        files.push(json!({
            "path": "style.css",
            "content": "body { font-family: system-ui, sans-serif; margin: 24px; } button { margin: 8px 0; }"
        }));
    }
    Ok(json!({
        "kind": kind,
        "files": files,
        "nextToolCall": {
            "tool": "app_register_inline",
            "arguments": {"files": "this structuredContent.files array", "dryRun": true}
        },
        "nextInstruction": "Call app_register_inline with dryRun:true next. Modify this complete files array first if needed, but do not print the app as prose/code before the dry run.",
        "next": [
            "For MCP-only clients, pass files to app_register_inline with dryRun:true, then commit with the same files.",
            "Pass files as the structuredContent.files JSON array, not a JSON string.",
            "On every app_register_inline retry, include the complete files array; do not send only changed files.",
            "For filesystem clients, write files into a bundle directory, call app_bundle_validate, then app_register.",
            "For UI apps, call window.terrane.invoke('verb', 'arg1', 'arg2') with positional args; do not pass an array for multiple backend args.",
            "For non-trivial UI, keep browser behavior in ui.js and backend behavior in main.js.",
            "For optional KV indexes such as event_ids, copy kvGetOrNull and default missing values before JSON.parse."
        ],
        "uiContract": {
            "browserInvoke": "window.terrane.invoke('verb', 'arg1', 'arg2') sends positional backend string args.",
            "wrongForTwoArgs": "window.terrane.invoke('verb', [arg1, arg2]) sends one string arg and should not be used for two backend args.",
            "verify": "For interactive outcomes, verify the page loads and one user-visible flow works when a browser is available."
        },
        "kvPattern": "Use kvGetOrNull(kv, key) for optional/index reads and default null to [] before JSON.parse."
    })
    .to_string())
}

fn app_multicap_scaffold_json(
    app_id: &str,
    name: &str,
    kind: &str,
    with_ui: bool,
) -> Result<String, String> {
    let app_id_js = serde_json::to_string(app_id).unwrap_or_else(|_| "\"app\"".to_string());
    let name_js = serde_json::to_string(name).unwrap_or_else(|_| "\"App\"".to_string());
    let manifest = if with_ui {
        json!({
            "id": app_id,
            "name": name,
            "runtime": "js",
            "backend": "main.js",
            "ui": "index.html",
            "resources": ["kv", "crdt", "relational_db"]
        })
    } else {
        json!({
            "id": app_id,
            "name": name,
            "runtime": "js",
            "backend": "main.js",
            "resources": ["kv", "crdt", "relational_db"]
        })
    };
    let main_js = r#"function projectSpec() {
  return {
    specVersion: 1,
    schemaVersion: 1,
    fields: {
      tenantId: { type: "string", required: true, minLength: 1 },
      projectId: { type: "string", required: true, minLength: 1 },
      owner: { type: "string", required: true },
      title: { type: "string", required: true },
      status: { type: "string", required: true, enum: ["active", "archived"] },
      createdAt: { type: "string", required: true, format: "date-time" }
    },
    primaryKey: { partition: ["tenantId"], sort: ["projectId"] },
    indexes: {
      byStatus: {
        partition: ["tenantId", "status"],
        sort: ["createdAt", "projectId"],
        projection: { type: "all" }
      },
      byOwner: {
        partition: ["tenantId", "owner"],
        sort: ["createdAt", "projectId"],
        projection: { type: "all" }
      }
    },
    constraints: {},
    options: { unknownFields: "reject", defaultQueryLimit: 10, maxQueryLimit: 50, canonicalJson: true }
  };
}

function ensureProjectsTable(db) {
  db.defineTable("projects", JSON.stringify(projectSpec()));
}

function projectRow(title) {
  return {
    tenantId: "acme",
    projectId: "p1",
    owner: "Ada",
    title: title,
    status: "active",
    createdAt: "2026-06-29T00:00:00Z"
  };
}

function parseJson(raw, fallback) {
  if (raw == null || raw === "") {
    return fallback;
  }
  return JSON.parse(raw);
}

function kvGetOrNull(kv, key) {
  try {
    return kv.get(key);
  } catch (err) {
    if (String(err).indexOf("not found") !== -1) {
      return null;
    }
    throw err;
  }
}

function kvRmIfPresent(kv, key) {
  try {
    kv.rm(key);
  } catch (err) {
    if (String(err).indexOf("not found") === -1) {
      throw err;
    }
  }
}

function valueOrNull(value) {
  return value == null ? null : value;
}

function readSummary() {
  var kv = ctx.resource.kv;
  var crdt = ctx.resource.crdt;
  var db = ctx.resource.relational_db;
  ensureProjectsTable(db);
  var p1Raw = db.get("projects", JSON.stringify({ tenantId: "acme", projectId: "p1" }));
  var activeRaw = db.query("projects", "byStatus", JSON.stringify({
    partition: { tenantId: "acme", status: "active" },
    select: "rows",
    limit: 10
  }));
  return JSON.stringify({
    app: __APP_ID_JSON__,
    resources: Object.keys(ctx.resource).sort(),
    kv: {
      theme: valueOrNull(kvGetOrNull(kv, "settings/theme")),
      lastNote: valueOrNull(kvGetOrNull(kv, "last-note"))
    },
    crdt: {
      owner: valueOrNull(crdt.mapGet("profile", "owner")),
      profile: crdt.mapAll("profile"),
      events: crdt.listAll("events"),
      journal: valueOrNull(crdt.textGet("journal"))
    },
    relational: {
      tables: parseJson(db.tables(), []),
      active: parseJson(activeRaw, []),
      p1: parseJson(p1Raw, null)
    }
  });
}

function handle(input) {
  var verb = input[0] || "";
  var kv = ctx.resource.kv;
  var crdt = ctx.resource.crdt;
  var db = ctx.resource.relational_db;

  if (verb === "__actions__") {
    return JSON.stringify({
      app: __APP_ID_JSON__,
      title: __APP_NAME_JSON__,
      description: "Generated multi-cap audit app.",
      capabilities: ["app", "kv", "crdt", "relational_db", "replica"],
      resources: ["kv", "crdt", "relational_db"],
      notes: "Use capability_command/query for replica and app catalog checks; app resources exercise kv, crdt, and relational_db.",
      actions: [
        { verb: "seed", summary: "Write KV, CRDT, and relational_db state.", args: [{ name: "text", required: true, summary: "project title and note text" }], returns: "JSON summary" },
        { verb: "summary", summary: "Read KV, CRDT, and relational_db state.", args: [], returns: "JSON summary" },
        { verb: "clearKv", summary: "Delete KV note keys while leaving CRDT and relational_db state.", args: [], returns: "JSON summary" }
      ]
    });
  }

  if (verb === "seed") {
    var text = input.slice(1).join(" ") || "multicap seed";
    ensureProjectsTable(db);
    kv.set("settings/theme", "forest");
    kv.set("last-note", text);
    crdt.mapSet("profile", "owner", "Ada");
    crdt.mapSet("profile", "lastNote", text);
    crdt.listPush("events", "seed:" + text);
    var currentJournal = crdt.textGet("journal");
    if (currentJournal == null || currentJournal === "") {
      crdt.textInsert("journal", "0", "ready");
    }
    db.put("projects", JSON.stringify(projectRow(text)));
    return readSummary();
  }

  if (verb === "summary") {
    return readSummary();
  }

  if (verb === "clearKv") {
    kvRmIfPresent(kv, "settings/theme");
    kvRmIfPresent(kv, "last-note");
    return readSummary();
  }

  return "unknown verb: " + verb;
}
"#
    .replace("__APP_ID_JSON__", &app_id_js)
    .replace("__APP_NAME_JSON__", &name_js);
    let mut files = vec![
        json!({"path": "manifest.json", "content": manifest.to_string()}),
        json!({"path": "main.js", "content": main_js}),
    ];
    if with_ui {
        files.push(json!({
            "path": "index.html",
            "content": "<!doctype html><html><head><title>Terrane Multi-cap App</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><main><h1>Terrane Multi-cap App</h1><button id=\"summary\">Summary</button><pre id=\"out\"></pre></main><script src=\"ui.js\"></script></body></html>"
        }));
        files.push(json!({
            "path": "ui.js",
            "content": "document.getElementById('summary').onclick = async function () {\n  document.getElementById('out').textContent = await window.terrane.invoke('summary');\n};\n"
        }));
        files.push(json!({
            "path": "style.css",
            "content": "body { font-family: system-ui, sans-serif; margin: 24px; } button { margin: 8px 0; }"
        }));
    }
    Ok(json!({
        "kind": kind,
        "capabilitiesUsed": ["app", "kv", "crdt", "relational_db", "replica"],
        "files": files,
        "nextToolCall": {
            "tool": "app_register_inline",
            "arguments": {"files": "this structuredContent.files array", "dryRun": true}
        },
        "nextInstruction": "Call app_register_inline with dryRun:true next. Modify this complete files array first if needed, but do not print the app as prose/code before the dry run.",
        "next": [
            "For MCP-only clients, pass files to app_register_inline with dryRun:true, then commit with the same files.",
            "Pass files as the structuredContent.files JSON array, not a JSON string.",
            "On every app_register_inline retry, include the complete files array; do not send only changed files.",
            "Call capability_command help for replica.init, then replica.init, then capability_query replica.peer.",
            "Call capability_query app.exists for the new id.",
            "Call app_actions, then invoke seed, summary as a separate pre-clear read, clearKv, and summary as a separate final read.",
            "For UI apps, call window.terrane.invoke('verb', 'arg1', 'arg2') with positional args; do not pass an array for multiple backend args.",
            "For optional KV reads, use kvGetOrNull(kv, key) before JSON.parse or final summary reads."
        ],
        "uiContract": {
            "browserInvoke": "window.terrane.invoke('verb', 'arg1', 'arg2') sends positional backend string args.",
            "wrongForTwoArgs": "window.terrane.invoke('verb', [arg1, arg2]) sends one string arg and should not be used for two backend args.",
            "verify": "For interactive outcomes, verify the page loads and one user-visible flow works when a browser is available."
        },
        "kvPattern": "Use kvGetOrNull(kv, key) for optional/index reads and default null before JSON.parse."
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

fn app_register_inline_json(
    core: &mut HostCore,
    id_override: &str,
    name_override: &str,
    runtime_override: &str,
    files: Vec<InlineFile>,
    dry_run: bool,
) -> Result<String, String> {
    let info = inspect_inline_bundle(id_override, name_override, runtime_override, &files)?;
    if !info.errors.is_empty() {
        return Err(format!(
            "app_register_inline refused invalid bundle: {}",
            info.errors.join("; ")
        ));
    }
    let dest = crate::home_dir().join("apps").join(&info.id);
    let source = if dry_run {
        dest.to_string_lossy().to_string()
    } else {
        // Validate app.add before touching the bundle directory.
        let source = dest.to_string_lossy().to_string();
        let argv = app_add_args(&info, &source);
        crate::dry_run_on_core(core, "app.add", &argv)?;
        write_inline_bundle(&dest, &files)?;
        dest.canonicalize()
            .map_err(|e| format!("resolve inline bundle {}: {e}", dest.display()))?
            .to_str()
            .ok_or("inline bundle path is not valid UTF-8")?
            .to_string()
    };
    let argv = app_add_args(&info, &source);
    if dry_run {
        let outcome = crate::dry_run_on_core(core, "app.add", &argv)?;
        Ok(json!({
            "dryRun": true,
            "command": "app.add",
            "args": argv,
            "records": outcome.records,
            "app": {"id": info.id, "name": info.name, "runtime": info.runtime, "source": source},
            "warnings": info.warnings,
            "next": "Call app_register_inline again with the same complete files array and no dryRun to write the owned bundle and commit. Pass files as a JSON array, not a JSON string."
        })
        .to_string())
    } else {
        let outcome = crate::dispatch_on_core(core, "app.add", &argv)?;
        Ok(json!({
            "command": "app.add",
            "args": argv,
            "records": outcome.records.len(),
            "output": outcome.output,
            "app": {"id": info.id, "name": info.name, "runtime": info.runtime, "source": source},
            "warnings": info.warnings,
            "next": [
                {"tool": "list_apps", "arguments": {}},
                {"tool": "app_actions", "arguments": {"app": info.id}},
                "If you change and re-register this app, send the complete files array again, including manifest.json, backend, UI, and assets."
            ]
        })
        .to_string())
    }
}

fn app_add_args(info: &BundleInfo, source: &str) -> Vec<String> {
    vec![
        info.id.clone(),
        info.name.clone(),
        "--source".to_string(),
        source.to_string(),
        "--runtime".to_string(),
        info.runtime.clone(),
    ]
}

fn inspect_inline_bundle(
    id_override: &str,
    name_override: &str,
    runtime_override: &str,
    files: &[InlineFile],
) -> Result<BundleInfo, String> {
    let manifest_file = files
        .iter()
        .find(|file| file.path == "manifest.json")
        .ok_or_else(|| "app_register_inline requires a manifest.json file".to_string())?;
    let mut manifest = BundleManifest::deserialize_json(&manifest_file.content)
        .map_err(|e| format!("manifest.json: {e}"))?;
    if manifest.runtime.trim().is_empty() {
        manifest.runtime = "js".to_string();
    }
    let id = defaulted(id_override, &manifest.id).to_string();
    let name = defaulted(name_override, defaulted(&manifest.name, &id)).to_string();
    let runtime = defaulted(runtime_override, &manifest.runtime).to_string();
    let backend = manifest.backend.trim().to_string();
    let ui = manifest.ui.trim().to_string();
    let file_paths: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
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
            validate_inline_ref("manifest.backend", &backend, &file_paths, &mut errors);
        }
    }
    if !ui.is_empty() {
        validate_inline_ref("manifest.ui", &ui, &file_paths, &mut errors);
    } else {
        warnings.push("manifest.ui is omitted; app is backend-only over invoke/MCP".to_string());
    }
    if manifest.resources.is_empty() {
        warnings.push(
            "manifest.resources is empty; add resources such as \"kv\" when needed".to_string(),
        );
    }
    if !manifest.id.trim().is_empty() && manifest.id.trim() != id {
        warnings.push(format!(
            "manifest.id {:?} differs from registered id {:?}",
            manifest.id, id
        ));
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

fn validate_inline_ref(
    label: &str,
    reference: &str,
    files: &BTreeSet<&str>,
    errors: &mut Vec<String>,
) {
    if !is_safe_relative_path(reference) {
        errors.push(format!(
            "{label} must be a safe relative path inside the bundle, got {reference:?}"
        ));
        return;
    }
    if !files.contains(reference) {
        errors.push(format!(
            "{label} references missing file {reference:?}. app_register_inline retries must include the complete files array: manifest.json, backend, manifest.ui, ui.js/style.css, and any other referenced assets; do not send only changed files"
        ));
    }
}

fn write_inline_bundle(dest: &std::path::Path, files: &[InlineFile]) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|e| format!("replace inline bundle {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("create inline bundle {}: {e}", dest.display()))?;
    for file in files {
        let target = dest.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(&target, &file.content)
            .map_err(|e| format!("write inline file {}: {e}", target.display()))?;
    }
    Ok(())
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
        errors.push(format!(
            "{label} references missing file {reference:?}; include every file referenced by manifest.json before registering"
        ));
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
        TOOL_APP_REGISTER_INLINE => json!({
            "files": [
                {"path": "manifest.json", "content": "{\"id\":\"notes-demo\",\"name\":\"Notes Demo\",\"runtime\":\"js\",\"backend\":\"main.js\",\"resources\":[\"kv\"]}"},
                {"path": "main.js", "content": "function handle(input){return 'ok';}"}
            ],
            "dryRun": true
        }),
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
