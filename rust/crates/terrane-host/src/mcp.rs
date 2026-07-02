//! Shared MCP semantics for Terrane host transports.
//!
//! Stdio and HTTP are just transports. This module owns the JSON-RPC request
//! handling and MCP tool behavior so every host exposes the same list ->
//! discover -> act contract.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use nanoserde::{DeJson, SerJson};
use serde_json::{json, Map, Value};
use terrane_api::{
    mcp_prompts, mcp_resource_templates, mcp_resources, mcp_tools, MCP_PROTOCOL_VERSION,
    MCP_SERVER_INSTRUCTIONS, TOOL_APP_ACTIONS, TOOL_APP_BUILD_COMMIT, TOOL_APP_BUILD_DISCARD,
    TOOL_APP_BUILD_GET, TOOL_APP_BUILD_LIST, TOOL_APP_BUILD_PUT_FILE, TOOL_APP_BUILD_START,
    TOOL_APP_BUILD_VALIDATE, TOOL_APP_BUNDLE_VALIDATE, TOOL_APP_RECIPE, TOOL_APP_REGISTER,
    TOOL_APP_REGISTER_INLINE, TOOL_APP_SCAFFOLD, TOOL_CAPABILITIES_LIST, TOOL_CAPABILITY_COMMAND,
    TOOL_CAPABILITY_INFO, TOOL_CAPABILITY_QUERY, TOOL_INVOKE, TOOL_LIST_APPS,
    TOOL_PERMISSION_CANCEL, TOOL_PERMISSION_CHECK, TOOL_PERMISSION_REQUESTS, TOOL_WORKFLOWS_LIST,
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
    handle_json_rpc_with_source(core, raw, "mcp_stdio")
}

pub fn handle_json_rpc_with_source(
    core: &mut HostCore,
    raw: &str,
    permission_source: &str,
) -> Option<String> {
    handle_json_rpc_with_source_and_admin_base(
        core,
        raw,
        permission_source,
        crate::permission::DEFAULT_ADMIN_BASE_URL,
    )
}

pub fn handle_json_rpc_with_source_and_admin_base(
    core: &mut HostCore,
    raw: &str,
    permission_source: &str,
    admin_base_url: &str,
) -> Option<String> {
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
        Some("tools/call") => {
            id.map(|id| {
                tool_call(
                    core,
                    id,
                    field("params").unwrap_or("{}"),
                    permission_source,
                    admin_base_url,
                )
            })
        }
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

// ---------------------------------------------------------------------------
// In-session permission approval over MCP elicitation (stdio transport).
//
// `handle_json_rpc` above is transport-agnostic and unchanged (the web/HTTP host
// still uses it verbatim). These helpers let the stdio host turn a
// `permission_required` tool result into a server->client `elicitation/create`
// request, read the human's decision, and — on approval — grant in-process and
// retry. The untrusted model never gains a grant tool; approval is a human
// action carried on MCP's back-channel.
// ---------------------------------------------------------------------------

/// The JSON-RPC `method` of a message, if present and a string.
pub fn parsed_method(raw: &str) -> Option<String> {
    let fields = top_level_fields(raw);
    fields
        .iter()
        .find(|(k, _)| *k == "method")
        .and_then(|(_, v)| json_string_value(v))
        .map(str::to_string)
}

/// Whether the client's `initialize` params declared the `elicitation`
/// capability. Only meaningful for an `initialize` message.
pub fn initialize_declares_elicitation(raw: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("params")?
                .get("capabilities")?
                .get("elicitation")
                .cloned()
        })
        .is_some_and(|elicitation| !elicitation.is_null())
}

/// What an elicitation prompt needs, extracted from a `permission_required` tool
/// result. The request is already recorded `pending` by the invoke path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElicitInfo {
    pub request_id: String,
    pub app: String,
    pub app_name: String,
    pub missing_resources: Vec<String>,
    pub admin_url: String,
}

/// If a tool response carries a `permission_required` result, extract the fields
/// an elicitation needs. Returns `None` for ordinary results.
pub fn permission_required_from_tool_response(response: &str) -> Option<ElicitInfo> {
    let value: Value = serde_json::from_str(response).ok()?;
    let content = value.get("result")?.get("structuredContent")?;
    if content.get("type")?.as_str()? != "permission_required" {
        return None;
    }
    if content
        .get("requestStatus")
        .and_then(Value::as_str)
        .is_some_and(|status| status == "preview")
    {
        return None;
    }
    let string = |key: &str| {
        content
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(ElicitInfo {
        request_id: content.get("requestId")?.as_str()?.to_string(),
        app: string("app"),
        app_name: string("appName"),
        missing_resources: content
            .get("missingResources")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        admin_url: string("adminUrl"),
    })
}

/// Build the server->client `elicitation/create` request frame.
pub fn elicitation_create_frame(elicit_id: &str, info: &ElicitInfo) -> String {
    let resources = if info.missing_resources.is_empty() {
        "the requested resources".to_string()
    } else {
        info.missing_resources.join(", ")
    };
    let message = format!(
        "App \"{}\" ({}) needs access to {}. Approve granting it to the local owner? \
         (Deny to refuse; admin console: {})",
        info.app_name, info.app, resources, info.admin_url
    );
    let params = json!({
        "message": message,
        "requestedSchema": {
            "type": "object",
            "properties": {
                "decision": {
                    "type": "string",
                    "enum": ["approve", "deny"],
                    "description": "approve to grant the requested resources; deny to refuse"
                }
            },
            "required": ["decision"]
        }
    });
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"method":"elicitation/create","params":{}}}"#,
        json_str(elicit_id),
        params
    )
}

/// The human's decision on an elicitation, if `line` is the matching response.
/// Returns `None` when `line` is unrelated (a different id, or not a response).
/// A matching id with any non-`accept` action — decline, cancel, or an error —
/// is a [`ElicitDecision::Deny`], so the model can never fall through to success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElicitDecision {
    Approve,
    Deny,
}

pub fn elicitation_decision(line: &str, elicit_id: &str) -> Option<ElicitDecision> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("id").and_then(Value::as_str) != Some(elicit_id) {
        return None;
    }
    let Some(result) = value.get("result") else {
        return Some(ElicitDecision::Deny); // error response to our request
    };
    if result.get("action").and_then(Value::as_str) != Some("accept") {
        return Some(ElicitDecision::Deny); // decline / cancel
    }
    let decision = result
        .get("content")
        .and_then(|content| content.get("decision"))
        .and_then(Value::as_str)
        .unwrap_or("deny");
    Some(if decision == "approve" {
        ElicitDecision::Approve
    } else {
        ElicitDecision::Deny
    })
}

/// A JSON-RPC "busy" error for a *request* that arrives while an elicitation is
/// outstanding (a well-behaved client waits, but we must not hang a stray one).
/// `None` for a notification (no id), which is simply ignored.
pub fn busy_error(raw: &str) -> Option<String> {
    let fields = top_level_fields(raw);
    let id = fields
        .iter()
        .find(|(k, _)| *k == "id")
        .map(|(_, v)| *v)
        .filter(|v| *v != "null")?;
    Some(error(
        id,
        -32001,
        "terrane-mcp is awaiting an elicitation response; retry after the permission prompt is answered",
    ))
}

fn initialize_result() -> String {
    format!(
        r#"{{"protocolVersion":{},"capabilities":{{"tools":{{"listChanged":false}},"resources":{{"subscribe":false,"listChanged":false}},"prompts":{{"listChanged":false}}}},"serverInfo":{{"name":"terrane-mcp","version":"0.1.0"}},"instructions":{}}}"#,
        json_str(MCP_PROTOCOL_VERSION),
        json_str(MCP_SERVER_INSTRUCTIONS)
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
const MCP_DOC_AGENT_PLAYBOOK: &str = include_str!("../../../../host/mcp/docs/AGENT_PLAYBOOK.md");

fn resource_content(core: &mut HostCore, uri: &str) -> Result<(&'static str, String), String> {
    match uri {
        "terrane://docs/index" => Ok(("text/markdown", MCP_DOC_INDEX.to_string())),
        "terrane://docs/clients" => Ok(("text/markdown", MCP_DOC_CLIENTS.to_string())),
        "terrane://docs/app-building" => Ok(("text/markdown", MCP_DOC_APP_BUILDING.to_string())),
        "terrane://docs/capability-operations" => {
            Ok(("text/markdown", MCP_DOC_CAPABILITY_OPERATIONS.to_string()))
        }
        "terrane://docs/security" => Ok(("text/markdown", MCP_DOC_SECURITY.to_string())),
        "terrane://docs/agent-playbook" => {
            Ok(("text/markdown", MCP_DOC_AGENT_PLAYBOOK.to_string()))
        }
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
2. Call `app_build_start` with {{"id":{app_id_json},"name":{app_name_json},"withUi":true}}.
3. Use `app_build_put_file` for any changed files, one file at a time.
4. Call `app_build_validate`, then `app_build_commit` with the returned `draftId` and `validationToken`; do not resend file contents.
5. Call `list_apps`, then `app_actions` for {app_id_json}.
6. Invoke `write` with {text_json}, invoke `read`, invoke `clear`, then invoke `read` again.

Compatibility route: `app_scaffold` plus `app_register_inline` dry-run is still valid, but the dry-run returns `draftId`/`validationToken`; finish with `app_build_commit` instead of resending the same complete files array.

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
fn tool_call(
    core: &mut HostCore,
    id: &str,
    params_raw: &str,
    permission_source: &str,
    admin_base_url: &str,
) -> String {
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
        TOOL_APP_BUILD_START => {
            let args = match args_object(TOOL_APP_BUILD_START, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let app_id = match required_string(args, "id", TOOL_APP_BUILD_START) {
                Ok(app_id) => app_id,
                Err(e) => return tool_text(id, &e, true),
            };
            let name = match required_string(args, "name", TOOL_APP_BUILD_START) {
                Ok(name) => name,
                Err(e) => return tool_text(id, &e, true),
            };
            let kind = match optional_string(args, "kind", TOOL_APP_BUILD_START) {
                Ok(kind) => kind,
                Err(e) => return tool_text(id, &e, true),
            };
            let with_ui = match optional_bool(args, "withUi", TOOL_APP_BUILD_START) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_build_start_json(&app_id, &name, &kind, with_ui) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_APP_BUILD_PUT_FILE => {
            let args = match args_object(TOOL_APP_BUILD_PUT_FILE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let draft_id = match required_string(args, "draftId", TOOL_APP_BUILD_PUT_FILE) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            if let Some(files_value) = args.get("files") {
                let files =
                    match inline_files_from_value(files_value, "files", TOOL_APP_BUILD_PUT_FILE) {
                        Ok(files) => files,
                        Err(e) => return tool_text(id, &e, true),
                    };
                return match app_build_put_files_json(&draft_id, &files) {
                    Ok(output) => tool_json(id, &output, false),
                    Err(e) => build_error(id, TOOL_APP_BUILD_PUT_FILE, &draft_id, &e),
                };
            }
            let path = match required_string(args, "path", TOOL_APP_BUILD_PUT_FILE) {
                Ok(value) => value,
                Err(e) => {
                    return tool_text(
                        id,
                        &format!("{e} (or pass files as an array of {{path,content}} objects to write several files in one call)"),
                        true,
                    )
                }
            };
            let content =
                match required_string_allow_empty(args, "content", TOOL_APP_BUILD_PUT_FILE) {
                    Ok(value) => value,
                    Err(e) => return tool_text(id, &e, true),
                };
            match app_build_put_file_json(&draft_id, &path, &content) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => build_error(id, TOOL_APP_BUILD_PUT_FILE, &draft_id, &e),
            }
        }
        TOOL_APP_BUILD_GET => {
            let args = match args_object(TOOL_APP_BUILD_GET, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let draft_id = match required_string(args, "draftId", TOOL_APP_BUILD_GET) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let path = match optional_string(args, "path", TOOL_APP_BUILD_GET) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let include_content = match optional_bool(args, "includeContent", TOOL_APP_BUILD_GET) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_build_get_json(&draft_id, &path, include_content) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => build_error(id, TOOL_APP_BUILD_GET, &draft_id, &e),
            }
        }
        TOOL_APP_BUILD_LIST => {
            if let Err(e) = args_object(TOOL_APP_BUILD_LIST, &params.arguments) {
                return tool_text(id, &e, true);
            }
            match app_build_list_json() {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_APP_BUILD_VALIDATE => {
            let args = match args_object(TOOL_APP_BUILD_VALIDATE, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let draft_id = match required_string(args, "draftId", TOOL_APP_BUILD_VALIDATE) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_build_validate_json(core, &draft_id) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => build_error(id, TOOL_APP_BUILD_VALIDATE, &draft_id, &e),
            }
        }
        TOOL_APP_BUILD_COMMIT => {
            let args = match args_object(TOOL_APP_BUILD_COMMIT, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let draft_id = match required_string(args, "draftId", TOOL_APP_BUILD_COMMIT) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            let validation_token =
                match optional_string(args, "validationToken", TOOL_APP_BUILD_COMMIT) {
                    Ok(value) => value,
                    Err(e) => return tool_text(id, &e, true),
                };
            let replace_existing =
                match optional_bool(args, "replaceExisting", TOOL_APP_BUILD_COMMIT) {
                    Ok(value) => value,
                    Err(e) => return tool_text(id, &e, true),
                };
            match app_build_commit_json(core, &draft_id, &validation_token, replace_existing) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => build_error(id, TOOL_APP_BUILD_COMMIT, &draft_id, &e),
            }
        }
        TOOL_APP_BUILD_DISCARD => {
            let args = match args_object(TOOL_APP_BUILD_DISCARD, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let draft_id = match required_string(args, "draftId", TOOL_APP_BUILD_DISCARD) {
                Ok(value) => value,
                Err(e) => return tool_text(id, &e, true),
            };
            match app_build_discard_json(&draft_id) {
                Ok(output) => tool_json(id, &output, false),
                Err(e) => build_error(id, TOOL_APP_BUILD_DISCARD, &draft_id, &e),
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
            match crate::app_actions_checked_with_admin_base_and_source(
                core,
                &app,
                admin_base_url,
                permission_source,
            ) {
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
            match crate::invoke_app_checked_with_admin_base_and_source(
                core,
                &app,
                &verb,
                &argv,
                admin_base_url,
                permission_source,
            ) {
                Ok(output) => tool_text(id, &output, false),
                Err(crate::InvokeFailure::PermissionRequired(required)) => {
                    tool_json(id, &required.serialize_json(), true)
                }
                Err(crate::InvokeFailure::Other(e)) => tool_text(id, &e, true),
            }
        }
        TOOL_PERMISSION_CHECK => {
            let args = match args_object(TOOL_PERMISSION_CHECK, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let request_id = match required_string(args, "requestId", TOOL_PERMISSION_CHECK) {
                Ok(request_id) => request_id,
                Err(e) => return tool_text(id, &e, true),
            };
            match crate::permission::permission_request_view(core, &request_id, admin_base_url) {
                Ok(Some(view)) => tool_json(id, &view.serialize_json(), false),
                Ok(None) => tool_text(id, "permission request not found", true),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_PERMISSION_CANCEL => {
            let args = match args_object(TOOL_PERMISSION_CANCEL, &params.arguments) {
                Ok(args) => args,
                Err(e) => return tool_text(id, &e, true),
            };
            let request_id = match required_string(args, "requestId", TOOL_PERMISSION_CANCEL) {
                Ok(request_id) => request_id,
                Err(e) => return tool_text(id, &e, true),
            };
            let reason = match optional_string(args, "reason", TOOL_PERMISSION_CANCEL) {
                Ok(reason) => reason,
                Err(e) => return tool_text(id, &e, true),
            };
            match crate::permission::cancel_permission_request(
                core,
                &request_id,
                &reason,
                admin_base_url,
            ) {
                Ok(Some(view)) => tool_json(id, &view.serialize_json(), false),
                Ok(None) => tool_text(id, "permission request not found", true),
                Err(e) => tool_text(id, &e, true),
            }
        }
        TOOL_PERMISSION_REQUESTS => {
            match args_object(TOOL_PERMISSION_REQUESTS, &params.arguments) {
                Ok(_) => match crate::permission::permission_requests(core, admin_base_url) {
                    Ok(response) => tool_json(id, &response.serialize_json(), false),
                    Err(e) => tool_text(id, &e, true),
                },
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
            match crate::public_authz::authorize_public_query(&capability, &query) {
                Ok(crate::public_authz::PublicQueryAuthz::Allow) => {
                    match crate::query_on_core(core, &capability, &query, &argv) {
                        Ok(value) => {
                            tool_json(id, &query_value_json(&capability, &query, value), false)
                        }
                        Err(e) => tool_text(id, &e, true),
                    }
                }
                Ok(crate::public_authz::PublicQueryAuthz::Refuse { reason }) => {
                    tool_text(id, &reason, true)
                }
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
            match crate::public_authz::authorize_public_command(core, &name, &argv) {
                Ok(crate::public_authz::PublicCommandAuthz::Allow) => {}
                Ok(crate::public_authz::PublicCommandAuthz::Refuse { reason }) => {
                    return tool_text(id, &reason, true);
                }
                Ok(crate::public_authz::PublicCommandAuthz::NeedsGrant { app, namespace }) => {
                    let operation = format!("capability_command:{name}");
                    let required = if dry_run {
                        crate::permission::preview_permission_required_for_namespace_with_admin_base(
                            core,
                            &app,
                            &namespace,
                            &operation,
                            permission_source,
                            admin_base_url,
                        )
                    } else {
                        crate::permission::request_permission_for_namespace_with_admin_base(
                            core,
                            &app,
                            &namespace,
                            &operation,
                            permission_source,
                            admin_base_url,
                        )
                    };
                    return match required {
                        Ok(Some(required)) => tool_json(id, &required.serialize_json(), true),
                        Ok(None) => {
                            if dry_run {
                                match crate::dry_run_public_on_core(core, &name, &argv) {
                                    Ok(outcome) => {
                                        tool_json(id, &command_dry_run_json(outcome.records), false)
                                    }
                                    Err(e) => tool_text(id, &e, true),
                                }
                            } else {
                                match crate::dispatch_public_on_core(core, &name, &argv) {
                                    Ok(outcome) => tool_json(
                                        id,
                                        &command_outcome_json(
                                            outcome.records.len(),
                                            outcome.output.as_deref(),
                                        ),
                                        false,
                                    ),
                                    Err(e) => tool_text(id, &e, true),
                                }
                            }
                        }
                        Err(e) => tool_text(id, &e, true),
                    };
                }
                Err(e) => return tool_text(id, &e, true),
            }
            if dry_run {
                match crate::dry_run_public_on_core(core, &name, &argv) {
                    Ok(outcome) => tool_json(id, &command_dry_run_json(outcome.records), false),
                    Err(e) => tool_text(id, &e, true),
                }
            } else {
                match crate::dispatch_public_on_core(core, &name, &argv) {
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

fn required_string_allow_empty(
    args: &Map<String, Value>,
    field: &str,
    tool: &str,
) -> Result<String, String> {
    match args.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        None => Err(format!(
            "{tool} requires string '{field}'. Try: {}",
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
        return Err(string_vec_arg_error(args, field, tool));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(ToString::to_string)
                .ok_or_else(|| string_vec_arg_error(args, field, tool))
        })
        .collect()
}

fn string_vec_arg_error(args: &Map<String, Value>, field: &str, tool: &str) -> String {
    if tool == TOOL_INVOKE && field == "args" {
        let app = args.get("app").and_then(Value::as_str).unwrap_or("APP_ID");
        let verb = args.get("verb").and_then(Value::as_str).unwrap_or("VERB");
        return format!(
            "{tool} requires '{field}' to be a real JSON array of strings. \
             Do not pass a JSON-stringified array. For this call use \
             {{\"app\":{},\"verb\":{},\"args\":[\"<one string argument>\"]}}. \
             If the backend expects one JSON payload, wrap the JSON string once: \
             \"args\":[\"{{...}}\"], not \"args\":\"[...]\".",
            json_str(app),
            json_str(verb)
        );
    }
    format!(
        "{tool} requires '{field}' to be an array of strings. Try: {}",
        tool_call_example(tool)
    )
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
    inline_files_from_value(value, field, tool)
}

fn inline_files_from_value(
    value: &Value,
    field: &str,
    tool: &str,
) -> Result<Vec<InlineFile>, String> {
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
            {"when": "visible calendar, dashboard, form, natural-language input page, or other UI app backed by app state", "workflow": "make_js_kv_app_no_filesystem", "use": "app_build_start with withUi:true; the draft ships a working UI shell — keep style.css and edit main.js/ui.js"},
            {"when": "task asks for five surfaces, multiple capabilities, relational data, CRDT/collaboration, or replica identity", "workflow": "make_js_multicap_app_no_filesystem"},
            {"when": "bundle directory already exists", "workflow": "register_app_bundle"},
            {"when": "app already exists and the task is to operate it", "workflow": "inspect_app_actions"}
        ],
        "notes": [
            "Always call MCP tools through JSON-RPC method tools/call.",
            "Choose a workflow by matching the user's desired outcome to chooseByOutcome before acting.",
            "For locked-down or weak-model clients, prefer app_build_start, app_build_put_file, app_build_validate, then app_build_commit. This avoids resending a giant files array twice.",
            "app_build_commit takes draftId and validationToken; it does not need file contents.",
            "app_register_inline remains a compatibility bridge. Its dryRun returns draftId/validationToken so the next call can be app_build_commit.",
            "app_register_inline.files must be the structuredContent.files JSON array, not a JSON string.",
            "Every app_register_inline retry must include the complete files array: manifest.json, backend, manifest.ui, ui.js/style.css, and assets.",
            "After app_scaffold, either call app_register_inline with dryRun:true to create a draft bridge, or start over with app_build_start. Do not print the whole app as prose/code before the dry run.",
            "If a provider stalls with zero new output after workflow_info, capability_info, app_recipe, or app_scaffold, resume from the last structured result and make the nextToolCall/nextAfterScaffold call immediately.",
            "If permission_required appears, do not call capability_command auth.* or grant commands. Follow nextModelAction, poll permission_check, and retry the original call after trusted approval.",
            "For UI apps, window.terrane.invoke takes positional string args: invoke(\"verb\", \"arg1\", \"arg2\"), not invoke(\"verb\", [\"arg1\", \"arg2\"]).",
            "For optional KV indexes such as event_ids, use a kvGetOrNull helper and default missing keys to [] before JSON.parse.",
            "When the user asked for an interactive page, verify the page itself when possible; backend invoke success alone does not prove the UI works.",
            "Prefer app_register for app bundle registration; it validates the bundle and still dispatches app.add through core.",
            "JSON results include structuredContent and a text JSON copy for compatibility."
        ],
        "stallRecovery": [
            {"lastCompletedTool": "workflows_list", "nextToolCall": {"tool": "workflow_info", "arguments": {"name": "make_js_kv_app_no_filesystem"}}},
            {"lastCompletedTool": "workflow_info", "nextToolCall": "use workflow.nextAfterScaffold only after app_scaffold; otherwise call the next concrete step listed in workflow.steps"},
            {"lastCompletedTool": "capability_info", "nextToolCall": "return to the selected workflow.steps and call the first not-yet-completed concrete app tool"},
            {"lastCompletedTool": "app_recipe", "nextToolCall": "use recipe.nextToolCall or recipe.firstCalls[0]"},
            {"lastCompletedTool": "app_scaffold", "nextToolCall": {"tool": "app_register_inline", "arguments": {"files": "last app_scaffold structuredContent.files array", "dryRun": true}}, "then": {"tool": "app_build_commit", "arguments": {"draftId": "draftId from dryRun", "validationToken": "validationToken from dryRun"}}}
        ]
    })
    .to_string()
}

fn workflow_info_json(name: &str) -> Result<String, String> {
    let workflow = match name.trim() {
        "make_js_kv_app" => json!({
            "name": "make_js_kv_app",
            "goal": "Build and run a small JS app backed by kv.",
            "primaryFlow": "Use staged draft tools first: app_build_start, app_build_put_file for changed files, app_build_validate, app_build_commit. This avoids resending a full files array.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "filesArgument": {"from": "app_scaffold.structuredContent.files", "type": "array", "doNotJsonStringify": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array to create a draft bridge, then commit with app_build_commit using draftId/validationToken. Pass files as an array; do not JSON-stringify it. Do not emit the whole app as prose/code before the dry run."
            },
            "stallRecovery": {
                "classification": "If a new assistant stream produces no tokens after this workflow_info result, classify it as provider/client stall unless a tool error appears.",
                "resume": "Restart or resume by calling the first not-yet-completed concrete step below. If app_scaffold already completed, call nextAfterScaffold immediately."
            },
            "steps": [
                {"tool": "app_recipe", "arguments": {"kind": "js_kv_app"}, "why": "Read the happy path before doing work."},
                {"tool": "app_build_start", "arguments": {"id": "notes-demo", "name": "Notes Demo", "withUi": true}, "why": "Create a server-side draft and avoid sending the full bundle twice. Omit withUi only for backend-only tasks."},
                {"tool": "app_build_put_file", "arguments": {"draftId": "draftId from app_build_start", "path": "main.js", "content": "complete backend file"}, "why": "Replace one file at a time. Repeat for index.html, ui.js, style.css, or assets as needed."},
                {"tool": "app_build_validate", "arguments": {"draftId": "draftId from app_build_start"}, "why": "Validate bundle refs and dry-run app.add without appending records."},
                {"tool": "app_build_commit", "arguments": {"draftId": "draftId from app_build_start", "validationToken": "validationToken from app_build_validate"}, "why": "Write the owned bundle and commit app.add without resending files."},
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
            "stagedBuildContract": "app_build_start creates a draft; app_build_put_file updates one file; app_build_validate returns validationToken; app_build_commit commits using draftId and validationToken without file contents.",
            "inlineFilesContract": "Compatibility path: app_register_inline.files must be a JSON array of {path,content} objects. Its dryRun returns draftId/validationToken, so prefer app_build_commit after dryRun rather than resending files.",
            "replaceExisting": "If the id already exists, operate the existing app with app_actions unless the human explicitly wants replacement. Replacement/removal is trusted-operator-only now; untrusted capability_command app.remove is refused. Ask the operator to remove/replace out of band, then validate/commit the draft again or use the compatibility inline dry-run bridge.",
            "pathBundleAlternative": "If the client can write files itself, write app_scaffold files to a bundle directory, call app_bundle_validate, then app_register dryRun and commit."
        }),
        "make_js_kv_app_no_filesystem" => json!({
            "name": "make_js_kv_app_no_filesystem",
            "goal": "Create a JS kv app when read/list/glob/grep/bash are denied.",
            "primaryFlow": "Use app_build_start -> app_build_put_file -> app_build_validate -> app_build_commit. Do not use filesystem tools and do not resend full bundles.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "filesArgument": {"from": "app_scaffold.structuredContent.files", "type": "array", "doNotJsonStringify": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array to create a draft bridge, then call app_build_commit with the returned draftId/validationToken. Pass files as an array; do not JSON-stringify it. Do not emit the whole app as prose/code before the dry run."
            },
            "stallRecovery": {
                "classification": "If a new assistant stream produces no tokens after this workflow_info result, classify it as provider/client stall unless a tool error appears.",
                "resume": "Restart or resume by calling app_build_start. If a draft already exists, recover it with app_build_list and continue."
            },
            "steps": [
                {"tool": "app_build_start", "arguments": {"id": "notes-demo", "name": "Notes Demo", "withUi": true}, "why": "Create a server-side draft. With withUi:true the draft is a working app shell: index.html regions, ui.js helpers, and a full style.css design system. Omit withUi only for backend-only tasks."},
                {"tool": "app_build_put_file", "arguments": {"draftId": "draftId from app_build_start", "path": "main.js", "content": "complete backend file"}, "why": "Put main.js first; only send index.html/ui.js/style.css if you changed them — the shell versions are already in the draft. Keep style.css unless you need custom components."},
                {"tool": "app_build_validate", "arguments": {"draftId": "draftId from app_build_start"}, "why": "Validate without writing files or committing."},
                {"tool": "app_build_commit", "arguments": {"draftId": "draftId from app_build_start", "validationToken": "validationToken from app_build_validate"}, "why": "Write under TERRANE_HOME/apps/<id> and commit through app.add without resending files."},
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
            "stagedBuildContract": "app_build_start creates a server-side draft; app_build_put_file updates one file; app_build_validate returns validationToken; app_build_commit commits without file contents.",
            "inlineFilesContract": "Compatibility path: app_register_inline.files must be a JSON array of {path,content} objects. Its dryRun returns draftId/validationToken, so prefer app_build_commit after dryRun rather than resending files.",
            "replaceExisting": "If the id already exists, operate it with app_actions unless the human explicitly wants replacement. Replacement/removal is trusted-operator-only now; untrusted capability_command app.remove is refused. Ask the operator to remove/replace out of band, then validate/commit the draft again or use the compatibility inline dry-run bridge.",
            "doNotUse": ["source reads", "shell", "glob", "grep", "filesystem list", "capability_command app.add before app_build_commit", "backend-only proof for visible UI tasks"]
        }),
        "make_js_multicap_app_no_filesystem" => json!({
            "name": "make_js_multicap_app_no_filesystem",
            "goal": "Create and verify a backend JS app that uses five capability surfaces without filesystem/source access.",
            "primaryFlow": "Use app_build_start -> app_build_put_file -> app_build_validate -> app_build_commit for the app bundle, then use capability_query/command and app invoke for verification.",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "filesArgument": {"from": "app_scaffold.structuredContent.files", "type": "array", "doNotJsonStringify": true},
                "instruction": "After app_scaffold, call this immediately with the complete files array to create a draft bridge, then call app_build_commit with the returned draftId/validationToken. Pass files as an array; do not JSON-stringify it. Do not emit the whole app as prose/code before the dry run."
            },
            "stallRecovery": {
                "classification": "If a new assistant stream produces no tokens after this workflow_info result, classify it as provider/client stall unless a tool error appears.",
                "resume": "Restart or resume by calling the first not-yet-completed concrete step below. If app_scaffold already completed, call nextAfterScaffold immediately."
            },
            "capabilitiesUsed": [
                {"namespace": "app", "how": "app_build_commit, app_actions, invoke, and capability_query app.exists"},
                {"namespace": "kv", "how": "ctx.resource.kv inside the generated app"},
                {"namespace": "crdt", "how": "ctx.resource.crdt inside the generated app"},
                {"namespace": "relational_db", "how": "ctx.resource.relational_db inside the generated app"},
                {"namespace": "replica", "how": "capability_command replica.init and capability_query replica.peer"}
            ],
            "steps": [
                {"tool": "capability_info", "arguments": {"namespace": "kv", "format": "json"}, "why": "Review app-scoped KV methods and reserved key constraints."},
                {"tool": "capability_info", "arguments": {"namespace": "crdt", "format": "json"}, "why": "Review map/list/text resource methods."},
                {"tool": "capability_info", "arguments": {"namespace": "relational_db", "format": "json"}, "why": "Review table spec and query method shape."},
                {"tool": "app_build_start", "arguments": {"id": "multicap-demo", "name": "Multi-cap Demo", "kind": "js_multicap_audit"}, "why": "Create a server-side draft using resources kv, crdt, and relational_db."},
                {"tool": "app_build_put_file", "arguments": {"draftId": "draftId from app_build_start", "path": "main.js", "content": "complete backend file"}, "why": "Replace one file at a time if customizing the generated app."},
                {"tool": "app_build_validate", "arguments": {"draftId": "draftId from app_build_start"}, "why": "Validate app.add through core without writing files."},
                {"tool": "app_build_commit", "arguments": {"draftId": "draftId from app_build_start", "validationToken": "validationToken from app_build_validate"}, "why": "Write under TERRANE_HOME/apps/<id> and commit through app.add without resending files."},
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
            "stagedBuildContract": "app_build_start creates a server-side draft; app_build_put_file updates one file; app_build_validate returns validationToken; app_build_commit commits without file contents.",
            "inlineFilesContract": "Compatibility path: app_register_inline.files must be a JSON array of {path,content} objects. Its dryRun returns draftId/validationToken, so prefer app_build_commit after dryRun rather than resending files.",
            "kvOptionalReads": "Generated apps should use kvGetOrNull(kv, key) for optional KV state and default null before JSON.parse or final summary reads.",
            "successSignals": [
                "app.exists returns true",
                "replica.peer returns a u64 value",
                "seed output contains kv.lastNote, crdt.profile.owner, crdt.events, crdt.journal, relational.active, and relational.p1",
                "the separate pre-clear summary after seed contains kv.lastNote, crdt.profile.owner, crdt.events, crdt.journal, relational.active, and relational.p1",
                "the separate post-clear summary has kv.theme null and kv.lastNote null while relational.p1 and crdt.profile.owner still exist"
            ],
            "doNotUse": ["source reads", "shell", "glob", "grep", "filesystem list", "net/model effects", "capability_command app.add before app_build_commit"]
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
            "nextToolCall": {"tool": "workflow_info", "arguments": {"name": "make_js_multicap_app_no_filesystem"}},
            "nextModelAction": "Call workflow_info for make_js_multicap_app_no_filesystem next. If workflow_info was already read, call app_build_start with kind js_multicap_audit next, then app_build_validate and app_build_commit.",
            "stallRecovery": "If the provider emits no tokens after this app_recipe result, retry once from this result and make nextToolCall the first action.",
            "steps": [
                "Call workflow_info with make_js_multicap_app_no_filesystem for the complete five-capability route.",
                "If the model started from an outcome-only request, choose this recipe after workflows_list maps multi-cap, relational, CRDT, or replica tasks to make_js_multicap_app_no_filesystem.",
                "Call app_build_start with kind js_multicap_audit to create a server-side draft.",
                "Use app_build_put_file for any customized files, one file at a time.",
                "Call app_build_validate, then app_build_commit with draftId and validationToken to write under TERRANE_HOME/apps/<id> and commit through core app.add.",
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
            "backendContract": "main.js is ONE plain script: no top-level import/export, no require, no modules. Define one global function handle(input); input is an array of strings where input[0] is the verb and input.slice(1) are the args; return a string (JSON.stringify for structured data).",
            "nextAfterScaffold": {
                "tool": "app_register_inline",
                "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
                "filesArgument": {"from": "app_scaffold.structuredContent.files", "type": "array", "doNotJsonStringify": true},
                "instruction": "Compatibility bridge: after app_scaffold, call this with the complete files array to get draftId/validationToken, then app_build_commit without resending files."
            },
            "uiContract": {
                "when": "If the requested outcome includes a visible page, pass withUi:true to app_scaffold and keep index.html plus ui.js separate from main.js.",
                "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. Do not pass [arg1,arg2] for multiple backend args.",
                "verification": "For UI outcomes, page load and rendered results matter in addition to backend invoke checks."
            },
            "firstCalls": [
                {"tool": "workflow_info", "arguments": {"name": "make_js_multicap_app_no_filesystem"}},
                {"tool": "app_build_start", "arguments": {"id": "multicap-demo", "name": "Multi-cap Demo", "kind": "js_multicap_audit"}},
                {"tool": "app_build_validate", "arguments": {"draftId": "draftId from app_build_start"}},
                {"tool": "app_build_commit", "arguments": {"draftId": "draftId from app_build_start", "validationToken": "validationToken from app_build_validate"}}
            ]
        })
        .to_string();
    }
    json!({
        "kind": kind,
        "summary": "Happy path for building a small JS Terrane app.",
        "nextToolCall": {"tool": "app_build_start", "arguments": {"id": "notes-demo", "name": "Notes Demo", "withUi": true}},
        "nextModelAction": "Call app_build_start next, using withUi:true for visible calendars, dashboards, forms, or natural-language input pages. Then update files with app_build_put_file, call app_build_validate, and commit with app_build_commit.",
        "stallRecovery": "If the provider emits no tokens after this app_recipe result, retry once from this result and make nextToolCall the first action.",
        "steps": [
            "Call app_build_start with id/name to create a server-side draft. Add withUi:true for calendars, dashboards, forms, natural-language input pages, or any visible app.",
            "Use app_build_put_file for each file you change; send one file at a time.",
            "Call app_build_validate, then app_build_commit with draftId and validationToken to write under TERRANE_HOME/apps/<id> and commit through core app.add.",
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
        "backendContract": "main.js is ONE plain script: no top-level import/export, no require, no modules. Define one global function handle(input); input is an array of strings where input[0] is the verb and input.slice(1) are the args; return a string (JSON.stringify for structured data).",
        "stagedBuildContract": "app_build_start creates a server-side draft; app_build_put_file updates one file; app_build_validate returns validationToken; app_build_commit commits without file contents.",
        "inlineFilesContract": "Compatibility path: app_register_inline.files must be a JSON array of {path,content} objects. Its dryRun returns draftId/validationToken, so prefer app_build_commit after dryRun rather than resending files.",
        "nextAfterScaffold": {
            "tool": "app_register_inline",
            "arguments": {"files": "structuredContent.files from app_scaffold", "dryRun": true},
            "filesArgument": {"from": "app_scaffold.structuredContent.files", "type": "array", "doNotJsonStringify": true},
            "instruction": "Compatibility bridge: after app_scaffold, call this with the complete files array to get draftId/validationToken, then app_build_commit without resending files."
        },
        "uiContract": {
            "browserInvoke": "window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") sends positional backend string args. Do not use window.terrane.invoke(\"verb\", [arg1,arg2]) for two backend args.",
            "files": "The withUi scaffold ships a working shell: keep style.css (full light+dark design system) and the KEEP-marked ui.js helpers; edit the REPLACE-marked functions and main.js. Keep index.html mostly markup.",
            "verification": "When the requested outcome is a UI app, verify the page loads and one user-visible flow works; backend invoke checks alone are not enough."
        },
        "firstCalls": [
            {"tool": "app_build_start", "arguments": {"id": "notes-demo", "name": "Notes Demo", "withUi": true}},
            {"tool": "app_build_validate", "arguments": {"draftId": "draftId from app_build_start"}},
            {"tool": "app_build_commit", "arguments": {"draftId": "draftId from app_build_start", "validationToken": "validationToken from app_build_validate"}}
        ]
    })
    .to_string()
}

/// The withUi app-shell templates: a working generic item app with a real
/// design system, so weak models edit main.js and small ui.js deltas instead
/// of writing ~10KB of UI from scratch inside their output budget.
const SCAFFOLD_UI_INDEX_HTML: &str = include_str!("scaffold/js_kv_app/index.html");
const SCAFFOLD_UI_JS: &str = include_str!("scaffold/js_kv_app/ui.js");
const SCAFFOLD_UI_STYLE_CSS: &str = include_str!("scaffold/js_kv_app/style.css");
const SCAFFOLD_UI_MAIN_JS: &str = include_str!("scaffold/js_kv_app/main.js");

/// Escape arbitrary display text for interpolation into scaffold HTML — the
/// app `name` is free text, unlike the safe-id-checked `id`.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
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
            "unknown scaffold kind: {kind}. Use \"js_kv_app\" for interactive/UI apps, \"js_kv_notes\" for a minimal notes demo, or \"js_multicap_audit\" for a kv+crdt+relational_db proof."
        ));
    }
    // The UI shell is the js_kv_app scaffold; report it as such even when the
    // kind was defaulted so downstream labels match the emitted files.
    let kind = if with_ui { "js_kv_app" } else { kind };
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
    let main_js = if with_ui {
        SCAFFOLD_UI_MAIN_JS
            .replace("__APP_ID_JSON__", &app_id_js)
            .replace("__APP_NAME_JSON__", &name_js)
    } else {
        main_js
    };
    let mut files = vec![
        json!({"path": "manifest.json", "content": manifest.to_string()}),
        json!({"path": "main.js", "content": main_js}),
    ];
    if with_ui {
        let index_html = SCAFFOLD_UI_INDEX_HTML.replace("__APP_NAME__", &html_escape(name));
        files.push(json!({"path": "index.html", "content": index_html}));
        files.push(json!({"path": "ui.js", "content": SCAFFOLD_UI_JS}));
        files.push(json!({"path": "style.css", "content": SCAFFOLD_UI_STYLE_CSS}));
    }
    Ok(json!({
        "kind": kind,
        "files": files,
        "nextToolCall": {
            "tool": "app_register_inline",
            "arguments": {"files": "this structuredContent.files array", "dryRun": true}
        },
        "nextToolCallSource": {
            "tool": "app_register_inline",
            "filesArgument": {"from": "this_result.structuredContent.files", "type": "array", "doNotJsonStringify": true},
            "dryRun": true
        },
        "stallRecovery": {
            "classification": "If the client/provider starts a new assistant step after this app_scaffold result but emits zero output tokens and no tool call, this is a provider/client stall, not a Terrane rejection.",
            "retryBudget": "Retry this stalled run at most once before changing provider/model.",
            "resumeFirstToolCall": {"tool": "app_register_inline", "arguments": {"files": "this_result.structuredContent.files", "dryRun": true}}
        },
        "nextInstruction": "Call app_register_inline with dryRun:true next to create a draftId/validationToken bridge, then call app_build_commit without resending file contents. Modify this complete files array first if needed, but do not print the app as prose/code before the dry run.",
        "next": [
            "For MCP-only clients that already have this files array, pass it to app_register_inline with dryRun:true, then commit with app_build_commit using draftId/validationToken.",
            "Pass files as the structuredContent.files JSON array, not a JSON string.",
            "On every app_register_inline dry-run retry, include the complete files array; do not send only changed files.",
            "After a successful app_register_inline dry run, call app_build_commit; do not resend file contents unless you intentionally use the legacy fallback.",
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
        "nextToolCallSource": {
            "tool": "app_register_inline",
            "filesArgument": {"from": "this_result.structuredContent.files", "type": "array", "doNotJsonStringify": true},
            "dryRun": true
        },
        "stallRecovery": {
            "classification": "If the client/provider starts a new assistant step after this app_scaffold result but emits zero output tokens and no tool call, this is a provider/client stall, not a Terrane rejection.",
            "retryBudget": "Retry this stalled run at most once before changing provider/model.",
            "resumeFirstToolCall": {"tool": "app_register_inline", "arguments": {"files": "this_result.structuredContent.files", "dryRun": true}}
        },
        "nextInstruction": "Call app_register_inline with dryRun:true next to create a draftId/validationToken bridge, then call app_build_commit without resending file contents. Modify this complete files array first if needed, but do not print the app as prose/code before the dry run.",
        "next": [
            "For MCP-only clients that already have this files array, pass it to app_register_inline with dryRun:true, then commit with app_build_commit using draftId/validationToken.",
            "Pass files as the structuredContent.files JSON array, not a JSON string.",
            "On every app_register_inline dry-run retry, include the complete files array; do not send only changed files.",
            "After a successful app_register_inline dry run, call app_build_commit; do not resend file contents unless you intentionally use the legacy fallback.",
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

const MAX_DRAFT_FILE_BYTES: usize = 512 * 1024;
const MAX_DRAFT_TOTAL_BYTES: usize = 2 * 1024 * 1024;

fn app_build_start_json(
    app_id: &str,
    name: &str,
    kind: &str,
    with_ui: bool,
) -> Result<String, String> {
    let scaffold = app_scaffold_json(app_id, name, kind, with_ui)?;
    let scaffold: Value =
        serde_json::from_str(&scaffold).map_err(|e| format!("parse scaffold result: {e}"))?;
    let files_value = scaffold
        .get("files")
        .ok_or_else(|| "app_scaffold result did not include files".to_string())?;
    let files = inline_files_from_value(files_value, "files", TOOL_APP_BUILD_START)?;
    validate_draft_size(&files)?;
    let info = inspect_inline_bundle("", "", "", &files)?;
    if !info.errors.is_empty() {
        return Err(format!(
            "app_build_start produced invalid scaffold: {}",
            info.errors.join("; ")
        ));
    }
    let kind = scaffold
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("js_kv_notes");
    let draft_id = create_build_draft(kind, &files)?;
    let manifest_example = if with_ui {
        json!({"id": info.id, "name": info.name, "runtime": "js", "backend": "main.js", "ui": "index.html", "resources": ["kv"]})
    } else {
        json!({"id": info.id, "name": info.name, "runtime": "js", "backend": "main.js", "resources": ["kv"]})
    };
    let backend_contract = "main.js is ONE plain script: no top-level import/export, no require, no modules, no Deno/Node APIs. Define one global function handle(input); input is an array of strings where input[0] is the verb and input.slice(1) are the args; return a string (JSON.stringify for structured data). Storage is ctx.resource.kv; wrap kv.get in try/catch because missing keys throw.";
    let manifest_rules = "manifest.ui is a string file path, never an object; scripts and styles are referenced from index.html, not listed in the manifest.";
    let ui_contract = "Browser code calls window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") with positional string args and awaits the backend's string reply. Do not pass an args array or an object.";
    let contract = if with_ui {
        json!({
            "manifestExample": manifest_example,
            "manifestRules": manifest_rules,
            "backend": backend_contract,
            "ui": ui_contract,
            "filesToReplace": "The scaffold is a working app shell, not a placeholder. Typically replace only main.js (your verbs and data), then edit the REPLACE-marked functions in ui.js (renderItem, refresh, the submit handler). Keep the KEEP-marked invoke/status helpers.",
            "styleContract": "Keep style.css as-is unless you need custom components; it already provides a light+dark design system with .card, .btn, .badge, .tag, .list-item, .empty-state, .status, and .grid. Add new rules at the bottom instead of rewriting the file.",
            "uiShell": "index.html already has #input-form/#main-input, #status, #list, and #empty regions; keep the ids and edit text/labels rather than restructuring."
        })
    } else {
        json!({
            "manifestExample": manifest_example,
            "manifestRules": manifest_rules,
            "backend": backend_contract,
            "ui": ui_contract,
            "filesToReplace": "Scaffold files are placeholders for a template app. Replace the content of main.js with the requested app before validating."
        })
    };
    let next = if with_ui {
        json!([
            "Put main.js first (your verbs and data); only send index.html/ui.js/style.css if you changed them — unchanged scaffold files are already in the draft.",
            "Send one complete file per app_build_put_file call, or several at once via its files array.",
            "Call app_build_validate after editing.",
            "If validation succeeds, call app_build_commit with draftId and validationToken; do not resend the files."
        ])
    } else {
        json!([
            "Use app_build_put_file once per file you need to change; send one file at a time.",
            "Call app_build_validate after editing.",
            "If validation succeeds, call app_build_commit with draftId and validationToken; do not resend the files.",
            "For visible UI apps, keep backend behavior in main.js and browser behavior in ui.js."
        ])
    };
    Ok(json!({
        "draftId": draft_id,
        "kind": kind,
        "app": {"id": info.id, "name": info.name, "runtime": info.runtime},
        "files": file_summaries(&files),
        "bundleHash": validation_token(&files),
        "contract": contract,
        "nextToolCall": {
            "tool": "app_build_put_file",
            "arguments": {"draftId": draft_id, "path": "main.js", "content": "complete file content"}
        },
        "nextModelAction": "Reply with a tool call only: app_build_put_file carrying your complete main.js. Do not print the app as prose and do not read scaffold files back — the contract above is all you need.",
        "next": next,
        "stallRecovery": {
            "resume": "Call app_build_get with draftId to recover file summaries, then continue with app_build_put_file or app_build_validate.",
            "lostDraftId": "Call app_build_list to recover draft ids.",
            "commitAfterDryRun": "After app_build_validate succeeds, app_build_commit is a small call and does not require sending file contents again."
        }
    })
    .to_string())
}

/// Structured recovery for app_build_* tool errors — the same shape lesson as
/// `permission_required`: weak models resume from machine-readable
/// `nextToolCall` fields, not from prose. A lost/wrong draftId routes to
/// `app_build_list`; commit failures route back through validation.
fn build_error(id: &str, tool: &str, draft_id: &str, error: &str) -> String {
    let next_tool_call = if error.contains("draft not found") || draft_id.trim().is_empty() {
        json!({"tool": "app_build_list", "arguments": {}})
    } else if tool == TOOL_APP_BUILD_COMMIT {
        json!({"tool": "app_build_validate", "arguments": {"draftId": draft_id}})
    } else {
        json!({"tool": "app_build_get", "arguments": {"draftId": draft_id}})
    };
    let value = json!({
        "type": "build_error",
        "tool": tool,
        "error": error,
        "nextToolCall": next_tool_call
    });
    tool_value(id, error, &value, true)
}

/// Batch variant of app_build_put_file: validate every path and the resulting
/// bundle size first, then write — a bad entry rejects the whole call so the
/// draft never half-applies.
fn app_build_put_files_json(draft_id: &str, new_files: &[InlineFile]) -> Result<String, String> {
    if new_files.is_empty() {
        return Err(
            "app_build_put_file files array is empty; pass one or more {path,content} objects"
                .to_string(),
        );
    }
    let bundle = draft_bundle_dir(draft_id)?;
    ensure_draft_exists(draft_id, &bundle)?;
    let mut files = read_inline_bundle_files(&bundle)?;
    for file in new_files {
        if !is_safe_relative_path(&file.path) {
            return Err(format!(
                "app_build_put_file path must be a safe relative bundle path, got {:?}; no files were written",
                file.path
            ));
        }
        if file.content.len() > MAX_DRAFT_FILE_BYTES {
            return Err(format!(
                "app_build_put_file content for {:?} is {} bytes; limit is {MAX_DRAFT_FILE_BYTES}; no files were written",
                file.path,
                file.content.len()
            ));
        }
    }
    let mut merged = files.clone();
    for file in new_files {
        merged.retain(|existing| existing.path != file.path);
        merged.push(file.clone());
    }
    let new_total: usize = merged.iter().map(|file| file.content.len()).sum();
    if new_total > MAX_DRAFT_TOTAL_BYTES {
        return Err(format!(
            "app_build_put_file would make draft {new_total} bytes; total limit is {MAX_DRAFT_TOTAL_BYTES}; no files were written"
        ));
    }
    for file in new_files {
        write_draft_file(&bundle, &file.path, &file.content)?;
    }
    files = merged;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    write_draft_metadata(draft_id, "updated", &files)?;
    let written: Vec<Value> = new_files.iter().map(file_summary).collect();
    Ok(json!({
        "draftId": draft_id,
        "files": written,
        "bundleHash": validation_token(&files),
        "nextToolCall": {"tool": "app_build_validate", "arguments": {"draftId": draft_id}},
        "next": [
            "Continue app_build_put_file for any other changed files.",
            "When the draft is complete, call app_build_validate.",
            "Do not call app_build_commit until validation returns valid:true."
        ]
    })
    .to_string())
}

fn app_build_put_file_json(draft_id: &str, path: &str, content: &str) -> Result<String, String> {
    let bundle = draft_bundle_dir(draft_id)?;
    ensure_draft_exists(draft_id, &bundle)?;
    if !is_safe_relative_path(path) {
        return Err(format!(
            "app_build_put_file path must be a safe relative bundle path, got {path:?}"
        ));
    }
    if content.len() > MAX_DRAFT_FILE_BYTES {
        return Err(format!(
            "app_build_put_file content for {path:?} is {} bytes; limit is {MAX_DRAFT_FILE_BYTES}",
            content.len()
        ));
    }
    let mut files = read_inline_bundle_files(&bundle)?;
    let current_total: usize = files.iter().map(|file| file.content.len()).sum();
    let current_file_bytes = files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.content.len())
        .unwrap_or(0);
    let new_total = current_total - current_file_bytes + content.len();
    if new_total > MAX_DRAFT_TOTAL_BYTES {
        return Err(format!(
            "app_build_put_file would make draft {new_total} bytes; total limit is {MAX_DRAFT_TOTAL_BYTES}"
        ));
    }
    write_draft_file(&bundle, path, content)?;
    files.retain(|file| file.path != path);
    files.push(InlineFile {
        path: path.to_string(),
        content: content.to_string(),
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    write_draft_metadata(draft_id, "updated", &files)?;
    Ok(json!({
        "draftId": draft_id,
        "file": file_summary(&InlineFile { path: path.to_string(), content: content.to_string() }),
        "bundleHash": validation_token(&files),
        "nextToolCall": {"tool": "app_build_validate", "arguments": {"draftId": draft_id}},
        "next": [
            "Continue app_build_put_file for any other changed files.",
            "When the draft is complete, call app_build_validate.",
            "Do not call app_build_commit until validation returns valid:true."
        ]
    })
    .to_string())
}

fn app_build_get_json(draft_id: &str, path: &str, include_content: bool) -> Result<String, String> {
    let bundle = draft_bundle_dir(draft_id)?;
    ensure_draft_exists(draft_id, &bundle)?;
    let files = read_inline_bundle_files(&bundle)?;
    let initial = read_initial_hashes(draft_id);
    if !path.trim().is_empty() {
        if !is_safe_relative_path(path) {
            return Err(format!(
                "app_build_get path must be a safe relative bundle path, got {path:?}"
            ));
        }
        let Some(file) = files.iter().find(|file| file.path == path) else {
            return Err(format!("draft {draft_id} has no file {path:?}"));
        };
        let pristine = is_unmodified_scaffold(&initial, file);
        let mut value = json!({
            "draftId": draft_id,
            "file": file_summary(file),
            "unmodifiedScaffold": pristine,
            "bundleHash": validation_token(&files),
            "nextToolCall": {"tool": "app_build_put_file", "arguments": {"draftId": draft_id, "path": path, "content": "complete file content"}},
            "nextModelAction": "Reply with a tool call only (app_build_put_file or app_build_validate). Do not print file contents or the app as prose."
        });
        if pristine {
            value["note"] = json!("This file is still the unmodified scaffold shell. You do not need its content — the contract from app_build_start summarizes it. Write your main.js with app_build_put_file first.");
        }
        if include_content {
            value["content"] = json!(file.content);
        }
        return Ok(value.to_string());
    }
    let summaries: Vec<Value> = files
        .iter()
        .map(|file| {
            let mut summary = file_summary(file);
            summary["unmodifiedScaffold"] = json!(is_unmodified_scaffold(&initial, file));
            summary
        })
        .collect();
    Ok(json!({
        "draftId": draft_id,
        "files": summaries,
        "bundleHash": validation_token(&files),
        "metadata": read_draft_metadata(draft_id).unwrap_or_else(|_| json!({})),
        "nextToolCall": {"tool": "app_build_validate", "arguments": {"draftId": draft_id}},
        "nextModelAction": "Reply with a tool call only (app_build_put_file or app_build_validate). Do not read files marked unmodifiedScaffold:true and do not print the app as prose.",
        "next": [
            "Do not read files marked unmodifiedScaffold:true — they are the working shell and the contract from app_build_start summarizes them. Write your main.js first.",
            "Use app_build_get with includeContent:true and a path only for files you already changed.",
            "Use app_build_put_file to replace files, then call app_build_validate."
        ]
    })
    .to_string())
}

fn app_build_validate_json(core: &HostCore, draft_id: &str) -> Result<String, String> {
    let bundle = draft_bundle_dir(draft_id)?;
    ensure_draft_exists(draft_id, &bundle)?;
    let files = read_inline_bundle_files(&bundle)?;
    validate_draft_size(&files)?;
    let mut info = inspect_inline_bundle("", "", "", &files)?;
    // A pristine scaffold backend validates fine but is still the demo app —
    // run-5's resumed GLM validated a shell-only draft and stalled deciding
    // whether it was real. Say it outright.
    let initial = read_initial_hashes(draft_id);
    if let Some(backend_file) = files.iter().find(|file| file.path == info.backend) {
        if is_unmodified_scaffold(&initial, backend_file) {
            info.warnings.push(format!(
                "{} is still the unmodified scaffold demo app, not the requested app. Write your backend with app_build_put_file before committing.",
                info.backend
            ));
        }
    }
    let dest = crate::home_dir().join("apps").join(&info.id);
    if core.state().app.apps.contains_key(&info.id) {
        info.errors.push(format!(
            "app id {:?} already exists; app_build_commit is create-only. Operate it with app_actions or ask a trusted operator for replacement.",
            info.id
        ));
    }
    if dest.exists() && !core.state().app.apps.contains_key(&info.id) {
        info.errors.push(format!(
            "owned bundle path {} already exists without a catalog entry; choose a new app id or clean it up out of band",
            dest.display()
        ));
    }
    if !info.errors.is_empty() {
        return Ok(json!({
            "draftId": draft_id,
            "valid": false,
            "app": bundle_app_value(&info),
            "errors": info.errors,
            "warnings": info.warnings,
            "files": file_summaries(&files),
            "nextToolCall": {"tool": "app_build_put_file", "arguments": {"draftId": draft_id, "path": "manifest.json", "content": "fixed complete file content"}},
            "next": [
                "Fix each listed error with app_build_put_file, sending the complete corrected file.",
                "Then call app_build_validate again with the same draftId."
            ]
        })
        .to_string());
    }
    let source = dest.to_string_lossy().to_string();
    let argv = app_add_args(&info, &source);
    let outcome = crate::dry_run_on_core(core, "app.add", &argv)?;
    let token = validation_token(&files);
    write_draft_metadata(draft_id, "validated", &files)?;
    Ok(json!({
        "draftId": draft_id,
        "valid": true,
        "validationToken": token,
        "command": "app.add",
        "args": argv,
        "records": outcome.records,
        "app": bundle_app_value(&info),
        "warnings": info.warnings,
        "files": file_summaries(&files),
        "nextToolCall": {"tool": "app_build_commit", "arguments": {"draftId": draft_id, "validationToken": token}},
        "next": "Call app_build_commit with draftId and validationToken. Do not resend file contents."
    })
    .to_string())
}

fn app_build_commit_json(
    core: &mut HostCore,
    draft_id: &str,
    validation_token_arg: &str,
    replace_existing: bool,
) -> Result<String, String> {
    if replace_existing {
        return Err(
            "app_build_commit replaceExisting is reserved for a future trusted replace flow; current staged builds are create-only"
                .to_string(),
        );
    }
    let draft = draft_dir(draft_id)?;
    let bundle = draft.join("bundle");
    ensure_draft_exists(draft_id, &bundle)?;
    let files = read_inline_bundle_files(&bundle)?;
    validate_draft_size(&files)?;
    let info = inspect_inline_bundle("", "", "", &files)?;
    if !info.errors.is_empty() {
        return Err(format!(
            "app_build_commit refused invalid draft: {}. Call app_build_validate for structured details.",
            info.errors.join("; ")
        ));
    }
    let token = validation_token(&files);
    if !validation_token_arg.trim().is_empty() && validation_token_arg.trim() != token {
        return Err(format!(
            "app_build_commit validationToken does not match current draft; call app_build_validate again. expected current token {token}"
        ));
    }
    if core.state().app.apps.contains_key(&info.id) {
        return Err(format!(
            "app_build_commit refused existing app id {:?}; operate it with app_actions or use a future trusted replace flow",
            info.id
        ));
    }
    let apps_dir = crate::home_dir().join("apps");
    let dest = apps_dir.join(&info.id);
    if dest.exists() {
        return Err(format!(
            "app_build_commit refused because owned bundle path already exists: {}",
            dest.display()
        ));
    }
    let source = dest.to_string_lossy().to_string();
    let argv = app_add_args(&info, &source);
    crate::dry_run_on_core(core, "app.add", &argv)?;
    std::fs::create_dir_all(&apps_dir)
        .map_err(|e| format!("create app directory {}: {e}", apps_dir.display()))?;
    let tmp = apps_dir.join(format!(".{}.{}", info.id, draft_id));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)
            .map_err(|e| format!("remove stale temp app bundle {}: {e}", tmp.display()))?;
    }
    write_inline_bundle(&tmp, &files)?;
    std::fs::rename(&tmp, &dest).map_err(|e| {
        format!(
            "move validated draft into owned bundle {} -> {}: {e}",
            tmp.display(),
            dest.display()
        )
    })?;
    let source = dest
        .canonicalize()
        .map_err(|e| format!("resolve committed bundle {}: {e}", dest.display()))?
        .to_str()
        .ok_or("committed bundle path is not valid UTF-8")?
        .to_string();
    let argv = app_add_args(&info, &source);
    let outcome = crate::dispatch_on_core(core, "app.add", &argv)?;
    std::fs::remove_dir_all(&draft)
        .map_err(|e| format!("remove committed draft {}: {e}", draft.display()))?;
    Ok(json!({
        "command": "app.add",
        "args": argv,
        "records": outcome.records.len(),
        "output": outcome.output,
        "app": {"id": info.id, "name": info.name, "runtime": info.runtime, "source": source},
        "warnings": info.warnings,
        "draftId": draft_id,
        "draftDiscarded": true,
        "next": [
            {"tool": "list_apps", "arguments": {}},
            {"tool": "app_actions", "arguments": {"app": info.id}}
        ]
    })
    .to_string())
}

fn app_build_list_json() -> Result<String, String> {
    let root = crate::home_dir().join(".mcp-drafts");
    let mut drafts = Vec::new();
    if root.is_dir() {
        let entries = std::fs::read_dir(&root)
            .map_err(|e| format!("read drafts directory {}: {e}", root.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read drafts entry {}: {e}", root.display()))?;
            let name = entry.file_name();
            let Some(draft_id) = name.to_str() else {
                continue;
            };
            if !draft_id.starts_with("draft-") || !entry.path().join("bundle").is_dir() {
                continue;
            }
            let metadata = read_draft_metadata(draft_id).unwrap_or_else(|_| json!({}));
            drafts.push(json!({
                "draftId": draft_id,
                "kind": metadata.get("kind").cloned().unwrap_or(Value::Null),
                "app": metadata.get("app").cloned().unwrap_or(Value::Null),
                "updatedAtUnix": metadata.get("updatedAtUnix").cloned().unwrap_or(Value::Null),
                "bundleHash": metadata.get("bundleHash").cloned().unwrap_or(Value::Null),
                "files": metadata.get("files").cloned().unwrap_or(Value::Null)
            }));
        }
    }
    drafts.sort_by(|a, b| {
        let ta = a.get("updatedAtUnix").and_then(Value::as_u64).unwrap_or(0);
        let tb = b.get("updatedAtUnix").and_then(Value::as_u64).unwrap_or(0);
        tb.cmp(&ta).then_with(|| {
            let ia = a.get("draftId").and_then(Value::as_str).unwrap_or("");
            let ib = b.get("draftId").and_then(Value::as_str).unwrap_or("");
            ia.cmp(ib)
        })
    });
    if drafts.is_empty() {
        return Ok(json!({
            "drafts": [],
            "nextToolCall": {
                "tool": "app_build_start",
                "arguments": {"id": "my-app", "name": "My App", "kind": "js_kv_app", "withUi": true}
            },
            "next": "No drafts exist. Start one with app_build_start."
        })
        .to_string());
    }
    let newest = drafts[0].get("draftId").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "drafts": drafts,
        "nextToolCall": {"tool": "app_build_validate", "arguments": {"draftId": newest}},
        "nextModelAction": "Call app_build_validate with the newest draftId now; do not read files first. If it returns valid:true, call app_build_commit immediately.",
        "next": [
            "Drafts are newest-first. Resume by calling app_build_validate with your draftId; if it returns valid:true, commit immediately.",
            "Only read files that validation complains about (app_build_get); do not re-read unmodified scaffold files.",
            "Continue editing with app_build_put_file; discard drafts you no longer need with app_build_discard."
        ]
    })
    .to_string())
}

fn app_build_discard_json(draft_id: &str) -> Result<String, String> {
    let draft = draft_dir(draft_id)?;
    if !draft.exists() {
        return Err(format!("draft not found: {draft_id}"));
    }
    std::fs::remove_dir_all(&draft)
        .map_err(|e| format!("discard draft {}: {e}", draft.display()))?;
    Ok(json!({
        "draftId": draft_id,
        "discarded": true,
        "next": "Start a new draft with app_build_start if needed."
    })
    .to_string())
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
        let draft_id = create_build_draft("inline", &files)?;
        let token = validation_token(&files);
        Ok(json!({
            "dryRun": true,
            "command": "app.add",
            "args": argv,
            "records": outcome.records,
            "app": {"id": info.id, "name": info.name, "runtime": info.runtime, "source": source},
            "warnings": info.warnings,
            "draftId": draft_id,
            "validationToken": token,
            "nextToolCall": {"tool": "app_build_commit", "arguments": {"draftId": draft_id, "validationToken": token}},
            "next": [
                "Recommended: call app_build_commit with draftId and validationToken; do not resend the files.",
                "Compatibility fallback: call app_register_inline again with the same complete files array and no dryRun.",
                "Pass files as a JSON array, not a JSON string."
            ]
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
        .ok_or_else(|| "the bundle requires a manifest.json file".to_string())?;
    let mut manifest = match BundleManifest::deserialize_json(&manifest_file.content) {
        Ok(manifest) => manifest,
        Err(parse_err) => {
            return Ok(manifest_shape_info(
                &manifest_file.content,
                &parse_err.to_string(),
            ));
        }
    };
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
            if let Some(file) = files.iter().find(|file| file.path == backend) {
                check_js_backend_contract(&backend, &file.content, &mut errors, &mut warnings);
                if let Some(msg) = terrane_cap_js_runtime::js_script_syntax_error(&file.content) {
                    errors.push(format!(
                        "{backend} has a JavaScript syntax error: {msg}. Resend the COMPLETE file with app_build_put_file — this usually means the previous content was truncated."
                    ));
                }
            }
        }
    }
    if !ui.is_empty() {
        validate_inline_ref("manifest.ui", &ui, &file_paths, &mut errors);
        for file in files {
            if file.path == backend || !(file.path.ends_with(".js") || file.path.ends_with(".html"))
            {
                continue;
            }
            check_ui_invoke_arg_shape(&file.path, &file.content, &mut warnings);
            if file.path.ends_with(".js") {
                if let Some(msg) = terrane_cap_js_runtime::js_script_syntax_error(&file.content) {
                    errors.push(format!(
                        "{} has a JavaScript syntax error: {msg}. The page will break on load. Resend the COMPLETE file with app_build_put_file.",
                        file.path
                    ));
                }
                if file.content.contains("fetch(") && file.content.contains("/invoke") {
                    warnings.push(format!(
                        "{} fetches /invoke directly. Call window.terrane.invoke(\"verb\", \"arg1\", ...) instead — the bridge sends {{verb, args:[strings]}} and rejects objects in args.",
                        file.path
                    ));
                }
            }
        }
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

/// Run-2 evals caught a UI calling `invoke('verb', [a, b])` — the browser
/// bridge `String()`s each positional arg, so an array always reaches the
/// backend as one comma-joined string. Backend smoke tests can't see this
/// (they call the CLI directly), so flag it at validation time.
fn check_ui_invoke_arg_shape(path: &str, content: &str, warnings: &mut Vec<String>) {
    for (idx, line) in content.lines().enumerate() {
        let mut rest = line;
        while let Some(pos) = rest.find("invoke(") {
            let after = &rest[pos + "invoke(".len()..];
            if let Some(second_arg) = second_call_arg(after) {
                if second_arg.trim_start().starts_with('[') {
                    warnings.push(format!(
                        "{path} line {} passes an array to invoke(). window.terrane.invoke sends positional string args and String()-joins an array into one argument. Call invoke(\"verb\", \"arg1\", \"arg2\") instead.",
                        idx + 1
                    ));
                    return;
                }
            }
            rest = after;
        }
    }
}

/// The text after the first top-level comma inside one call's argument list
/// (single-line heuristic; quotes are not tracked — good enough for a warning).
fn second_call_arg(after_open_paren: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (i, ch) in after_open_paren.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            ',' if depth == 0 => return Some(&after_open_paren[i + 1..]),
            _ => {}
        }
    }
    None
}

const MANIFEST_EXAMPLE: &str = r#"{"id":"my-app","name":"My App","runtime":"js","backend":"main.js","ui":"index.html","resources":["kv"]}"#;

/// A weak-model-friendly `BundleInfo` for a manifest.json that failed strict
/// parsing: name the offending field types and show the exact accepted shape,
/// instead of surfacing a parser error the model cannot act on.
fn manifest_shape_info(raw: &str, parse_err: &str) -> BundleInfo {
    let mut errors = Vec::new();
    let mut id = String::new();
    let mut name = String::new();
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => {
            for (field, expected) in [
                ("id", "a string such as \"my-app\""),
                ("name", "a string such as \"My App\""),
                ("runtime", "the string \"js\""),
                ("backend", "a string file path such as \"main.js\""),
                (
                    "ui",
                    "a string file path such as \"index.html\" (scripts and styles are referenced from index.html, not listed in the manifest)",
                ),
            ] {
                match map.get(field) {
                    None | Some(Value::String(_)) => {}
                    Some(other) => errors.push(format!(
                        "manifest.{field} must be {expected}, not {}",
                        json_type_name(other)
                    )),
                }
            }
            match map.get("resources") {
                None => {}
                Some(Value::Array(items)) if items.iter().all(Value::is_string) => {}
                Some(_) => errors.push(
                    "manifest.resources must be an array of strings such as [\"kv\"]".to_string(),
                ),
            }
            if errors.is_empty() {
                errors.push(format!(
                    "manifest.json does not match the accepted manifest shape: {parse_err}"
                ));
            }
            id = map
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            name = map
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
        Ok(_) => errors.push("manifest.json must be a JSON object".to_string()),
        Err(e) => errors.push(format!("manifest.json is not valid JSON: {e}")),
    }
    errors.push(format!(
        "Fix: replace manifest.json with exactly this shape: {MANIFEST_EXAMPLE} (omit \"ui\" for backend-only apps)"
    ));
    BundleInfo {
        id,
        name,
        runtime: "js".to_string(),
        backend: String::new(),
        ui: String::new(),
        resources: Vec::new(),
        errors,
        warnings: Vec::new(),
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Weak-model evals showed runtime-incompatible backends (ES modules, missing
/// or object-style `handle`) validating cleanly and then failing on first
/// invoke. The runtime evals the backend as one plain script and calls the
/// global-object `handle(input)` with an array of strings (an `actions` table
/// is the sanctioned alternative — the prelude synthesizes `handle` from it),
/// so catch contract breaks here with fix-it guidance.
fn check_js_backend_contract(
    path: &str,
    content: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let mut module_line = None;
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        let is_module = trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("import\"")
            || trimmed.starts_with("import'")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("export{");
        if is_module {
            module_line = Some(idx + 1);
            break;
        }
    }
    if let Some(line) = module_line {
        errors.push(format!(
            "{path} line {line} uses top-level import/export, but Terrane js backends run as one plain script: no ES modules, no require, no Deno/Node APIs. Inline all code into {path} and define one global function handle(input)"
        ));
    }
    let defines_handle = content.contains("function handle(")
        || content.contains("var handle")
        || content.contains("handle = function")
        || content.contains("handle=function")
        || content.contains("globalThis.handle");
    let lexical_handle_only =
        !defines_handle && (content.contains("const handle") || content.contains("let handle"));
    let defines_actions = content.contains("var actions")
        || content.contains("const actions")
        || content.contains("let actions")
        || content.contains("actions =")
        || content.contains("actions=");
    if lexical_handle_only {
        errors.push(format!(
            "{path} declares handle with const/let, but the runtime reads handle from the global object, so it will not be found. Declare it as: function handle(input) {{ ... }}"
        ));
    } else if !defines_handle && !defines_actions {
        errors.push(format!(
            "{path} does not define the required entrypoint. Add: function handle(input) {{ var verb = input[0] || \"\"; ...; return \"ok\"; }} — input is an array of strings and the return value must be a string (use JSON.stringify for structured data)"
        ));
    }
    for marker in ["input.action", "input.verb", "input.command", "input.args"] {
        if content.contains(marker) {
            warnings.push(format!(
                "{path} reads {marker}, but handle(input) receives an array of strings, not an object: input[0] is the verb and input.slice(1) are the args. Dispatch with: var verb = input[0] || \"\";"
            ));
            break;
        }
    }
    // Models that replace main.js often drop the __actions__ branch, which
    // breaks app_actions/verb discovery for every client after install.
    if defines_handle && !defines_actions && !content.contains("__actions__") {
        warnings.push(format!(
            "{path} does not handle the \"__actions__\" verb, so app_actions/verb discovery will return \"unknown verb\" after install. Add a branch: if (verb === \"__actions__\") return JSON.stringify({{app: \"<id>\", title: \"<name>\", actions: [{{verb, summary, args: [{{name, required}}], returns}}, ...]}});"
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

/// Abandoned drafts otherwise accumulate forever under `.mcp-drafts/`; keep
/// the newest MAX_DRAFTS and evict the rest whenever a new draft is created.
const MAX_DRAFTS: usize = 16;

fn create_build_draft(kind: &str, files: &[InlineFile]) -> Result<String, String> {
    let draft_id = new_draft_id()?;
    let draft = draft_dir(&draft_id)?;
    let bundle = draft.join("bundle");
    write_inline_bundle(&bundle, files)?;
    write_draft_metadata(&draft_id, kind, files)?;
    write_initial_hashes(&draft_id, files)?;
    evict_stale_drafts();
    Ok(draft_id)
}

/// The per-file hashes at draft creation, written once and never updated —
/// lets app_build_get tell a model "this file is still the unmodified
/// scaffold; you don't need its content". Run-3 evals showed stall-prone
/// models burning their whole window reading every pristine shell file.
fn write_initial_hashes(draft_id: &str, files: &[InlineFile]) -> Result<(), String> {
    let mut map = serde_json::Map::new();
    for file in files {
        map.insert(file.path.clone(), json!(stable_hash_text(&file.content)));
    }
    let path = draft_dir(draft_id)?.join("initial.json");
    std::fs::write(&path, Value::Object(map).to_string())
        .map_err(|e| format!("write draft initial hashes {}: {e}", path.display()))
}

fn read_initial_hashes(draft_id: &str) -> Value {
    draft_dir(draft_id)
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join("initial.json")).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}))
}

fn is_unmodified_scaffold(initial: &Value, file: &InlineFile) -> bool {
    initial
        .get(&file.path)
        .and_then(Value::as_str)
        .is_some_and(|hash| hash == stable_hash_text(&file.content))
}

/// Best-effort eviction of the oldest drafts beyond MAX_DRAFTS — hygiene must
/// never fail the draft that was just created, so errors are swallowed.
fn evict_stale_drafts() {
    let root = crate::home_dir().join(".mcp-drafts");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut drafts: Vec<(u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(draft_id) = name.to_str() else {
            continue;
        };
        if !draft_id.starts_with("draft-") {
            continue;
        }
        let updated = read_draft_metadata(draft_id)
            .ok()
            .and_then(|meta| meta.get("updatedAtUnix").and_then(Value::as_u64))
            .or_else(|| {
                entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
            })
            .unwrap_or(0);
        drafts.push((updated, entry.path()));
    }
    if drafts.len() <= MAX_DRAFTS {
        return;
    }
    drafts.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, path) in drafts.into_iter().skip(MAX_DRAFTS) {
        let _ = std::fs::remove_dir_all(&path);
    }
}

fn new_draft_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| format!("failed to read OS entropy for app build draft id: {e}"))?;
    Ok(format!("draft-{}", hex_bytes(&bytes)))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn draft_dir(draft_id: &str) -> Result<PathBuf, String> {
    validate_safe_id(draft_id)?;
    if !draft_id.starts_with("draft-") {
        return Err(format!(
            "draft id {draft_id:?} is invalid; expected an id returned by app_build_start"
        ));
    }
    Ok(crate::home_dir().join(".mcp-drafts").join(draft_id))
}

fn draft_bundle_dir(draft_id: &str) -> Result<PathBuf, String> {
    Ok(draft_dir(draft_id)?.join("bundle"))
}

fn ensure_draft_exists(draft_id: &str, bundle: &Path) -> Result<(), String> {
    if !bundle.is_dir() {
        return Err(format!(
            "draft not found: {draft_id}. Start one with app_build_start or recover a draftId from app_register_inline dryRun."
        ));
    }
    Ok(())
}

fn validate_draft_size(files: &[InlineFile]) -> Result<(), String> {
    let mut total = 0usize;
    for file in files {
        let bytes = file.content.len();
        if bytes > MAX_DRAFT_FILE_BYTES {
            return Err(format!(
                "draft file {:?} is {bytes} bytes; per-file limit is {MAX_DRAFT_FILE_BYTES}",
                file.path
            ));
        }
        total += bytes;
    }
    if total > MAX_DRAFT_TOTAL_BYTES {
        return Err(format!(
            "draft bundle is {total} bytes; total limit is {MAX_DRAFT_TOTAL_BYTES}"
        ));
    }
    Ok(())
}

fn write_draft_file(bundle: &Path, path: &str, content: &str) -> Result<(), String> {
    let target = bundle.join(path);
    let parent = target
        .parent()
        .ok_or_else(|| format!("draft file {path:?} has no parent directory"))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let tmp_name = format!(
        ".{}.tmp-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("draft-file"),
        std::process::id()
    );
    let tmp = parent.join(tmp_name);
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| format!("create temp draft file {}: {e}", tmp.display()))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("write temp draft file {}: {e}", tmp.display()))?;
        file.flush()
            .map_err(|e| format!("flush temp draft file {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| {
        format!(
            "replace draft file {} -> {}: {e}",
            tmp.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn read_inline_bundle_files(bundle: &Path) -> Result<Vec<InlineFile>, String> {
    if !bundle.is_dir() {
        return Err(format!("bundle directory not found: {}", bundle.display()));
    }
    let mut files = Vec::new();
    collect_inline_bundle_files(bundle, bundle, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.is_empty() {
        return Err(format!("bundle has no files: {}", bundle.display()));
    }
    Ok(files)
}

fn collect_inline_bundle_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<InlineFile>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("read bundle dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read bundle entry {}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read file type {}: {e}", path.display()))?;
        if file_type.is_dir() {
            collect_inline_bundle_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|e| format!("strip bundle prefix {}: {e}", path.display()))?;
        let rel = rel
            .to_str()
            .ok_or_else(|| format!("bundle path is not valid UTF-8: {}", path.display()))?
            .to_string();
        if !is_safe_relative_path(&rel) {
            return Err(format!("unsafe bundle path found in draft: {rel:?}"));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("read draft file {} as UTF-8 text: {e}", path.display()))?;
        files.push(InlineFile { path: rel, content });
    }
    Ok(())
}

fn write_draft_metadata(draft_id: &str, kind: &str, files: &[InlineFile]) -> Result<(), String> {
    let draft = draft_dir(draft_id)?;
    std::fs::create_dir_all(&draft)
        .map_err(|e| format!("create draft {}: {e}", draft.display()))?;
    let inspected = inspect_inline_bundle("", "", "", files);
    let (app, errors, warnings) = match inspected {
        Ok(info) => (bundle_app_value(&info), info.errors, info.warnings),
        Err(e) => (json!({"inspectError": e}), Vec::new(), Vec::new()),
    };
    let metadata = json!({
        "draftId": draft_id,
        "kind": kind,
        "updatedAtUnix": current_unix_secs(),
        "bundleHash": validation_token(files),
        "app": app,
        "errors": errors,
        "warnings": warnings,
        "files": file_summaries(files)
    });
    let metadata_path = draft.join("draft.json");
    std::fs::write(&metadata_path, metadata.to_string())
        .map_err(|e| format!("write draft metadata {}: {e}", metadata_path.display()))?;
    Ok(())
}

fn read_draft_metadata(draft_id: &str) -> Result<Value, String> {
    let path = draft_dir(draft_id)?.join("draft.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("read draft metadata {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse draft metadata {}: {e}", path.display()))
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn file_summaries(files: &[InlineFile]) -> Vec<Value> {
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.iter().map(file_summary).collect()
}

fn file_summary(file: &InlineFile) -> Value {
    json!({
        "path": file.path,
        "bytes": file.content.len(),
        "hash": stable_hash_text(&file.content)
    })
}

fn bundle_app_value(info: &BundleInfo) -> Value {
    json!({
        "id": info.id,
        "name": info.name,
        "runtime": info.runtime,
        "backend": info.backend,
        "ui": info.ui,
        "resources": info.resources
    })
}

fn validation_token(files: &[InlineFile]) -> String {
    format!("v1-{:016x}", stable_hash_files(files))
}

fn stable_hash_text(text: &str) -> String {
    format!("{:016x}", stable_hash_bytes(text.as_bytes()))
}

fn stable_hash_files(files: &[InlineFile]) -> u64 {
    let mut refs: Vec<&InlineFile> = files.iter().collect();
    refs.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = FNV_OFFSET;
    for file in refs {
        hash = stable_hash_update(hash, file.path.as_bytes());
        hash = stable_hash_update(hash, &[0xff]);
        hash = stable_hash_update(hash, file.content.as_bytes());
        hash = stable_hash_update(hash, &[0xfe]);
    }
    hash
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn stable_hash_bytes(bytes: &[u8]) -> u64 {
    stable_hash_update(FNV_OFFSET, bytes)
}

fn stable_hash_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
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
        TOOL_APP_BUILD_START => json!({"id": "notes-demo", "name": "Notes Demo", "withUi": true}),
        TOOL_APP_BUILD_PUT_FILE => {
            json!({"draftId": "draft-...", "path": "main.js", "content": "function handle(input){return 'ok';}"})
        }
        TOOL_APP_BUILD_GET => {
            json!({"draftId": "draft-...", "path": "main.js", "includeContent": true})
        }
        TOOL_APP_BUILD_VALIDATE => json!({"draftId": "draft-..."}),
        TOOL_APP_BUILD_COMMIT => {
            json!({"draftId": "draft-...", "validationToken": "v1-..."})
        }
        TOOL_APP_BUILD_DISCARD => json!({"draftId": "draft-..."}),
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
        TOOL_PERMISSION_CHECK => json!({"requestId": "REQUEST_ID"}),
        TOOL_PERMISSION_CANCEL => json!({"requestId": "REQUEST_ID", "reason": "not needed"}),
        TOOL_PERMISSION_REQUESTS => json!({}),
        TOOL_CAPABILITIES_LIST => json!({}),
        TOOL_CAPABILITY_INFO => json!({"namespace": "app", "format": "json"}),
        TOOL_CAPABILITY_QUERY => {
            json!({"capability": "app", "query": "exists", "args": ["APP_ID"]})
        }
        TOOL_CAPABILITY_COMMAND => json!({"name": "app.add", "help": true}),
        TOOL_LIST_APPS | TOOL_WORKFLOWS_LIST | TOOL_APP_BUILD_LIST => json!({}),
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
