//! terrane-api — the host API contract.
//!
//! The single source of the surface that terrane's edge hosts expose: the **web
//! host** (HTTP) and the **MCP host** (stdio JSON-RPC). It is the OSS-side typed
//! implementation of the contract that `terrane-premium` consumes as a pinned
//! `public-contract.json` (premium is a hosted *control plane*, not a host — it
//! pins and verifies this contract but does not serve these routes/tools). Kept
//! dependency-light (just nanoserde) so it stays a clean, vendorable contract.
//!
//! What lives here: the wire types (request/response JSON), the route table, the
//! MCP tool descriptors, and [`host_contract`] — the serializable summary that
//! the `terrane contract export` step folds into `public-contract.json`.
//!
//! What does NOT live here: any I/O, any HTTP/MCP server, any dependency on
//! `terrane-core`. The hosts implement this; the core knows nothing of it.

use nanoserde::{DeJson, SerJson};

/// Version of *this* host API surface. Bumped when a route/tool/shape changes.
pub const CONTRACT_VERSION: &str = "0.7.0";

/// The MCP protocol revision the MCP host speaks in its `initialize` handshake.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

// ---------------------------------------------------------------------------
// HTTP routes (web host)
// ---------------------------------------------------------------------------

/// `GET` — liveness. Returns [`HealthResponse`].
pub const ROUTE_HEALTHZ: &str = "/healthz";
/// `GET` — the installed app catalog. Returns [`AppsResponse`].
pub const ROUTE_APPS: &str = "/apps";
/// `POST` — MCP JSON-RPC over HTTP. Returns an MCP JSON-RPC response, or 202
/// for notifications with no response.
pub const ROUTE_MCP: &str = "/mcp";

/// The UI entry route for an app: `GET /apps/{id}/` (and `/apps/{id}/{asset}`).
pub fn route_app_ui(id: &str) -> String {
    format!("/apps/{id}/")
}

/// The invoke route for an app: `POST /apps/{id}/invoke` with an
/// [`InvokeRequest`] body, returning [`InvokeResponse`] (or [`ApiError`]).
pub fn route_app_invoke(id: &str) -> String {
    format!("/apps/{id}/invoke")
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// `GET /healthz`.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// One catalog entry in [`AppsResponse`].
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct AppSummary {
    pub id: String,
    pub name: String,
    /// Whether the app ships a UI (`manifest.ui`) the web host can serve.
    pub has_ui: bool,
}

/// `GET /apps`.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct AppsResponse {
    pub apps: Vec<AppSummary>,
}

/// `POST /apps/{id}/invoke` body — the HTTP twin of `window.terrane.invoke` and
/// of the MCP `invoke` tool: a verb plus its string argument array, run against
/// the app's backend runtime.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct InvokeRequest {
    pub verb: String,
    /// Optional — a verb with no arguments may omit it. This matches the MCP
    /// `invoke` tool's schema (`required: [app, verb]`), so the HTTP and MCP
    /// shapes agree.
    #[nserde(default)]
    pub args: Vec<String>,
}

/// A successful invoke — the backend's returned string.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct InvokeResponse {
    pub output: String,
}

/// A uniform error body for any failing request.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct ApiError {
    pub error: String,
}

// ---------------------------------------------------------------------------
// MCP tools (mcp host)
// ---------------------------------------------------------------------------

/// MCP tool: list the installed apps (so an agent can *select* one).
pub const TOOL_LIST_APPS: &str = "list_apps";
/// MCP tool: describe an app's actions, as the app itself declares them (so an
/// agent can *discover* what it can do before acting).
pub const TOOL_APP_ACTIONS: &str = "app_actions";
/// MCP tool: run a verb on an app (so an agent can *act* on it).
pub const TOOL_INVOKE: &str = "invoke";
/// MCP tool: check the status of a permission request.
pub const TOOL_PERMISSION_CHECK: &str = "permission_check";
/// MCP tool: cancel a pending permission request created by this host.
pub const TOOL_PERMISSION_CANCEL: &str = "permission_cancel";
/// MCP tool: list permission requests visible to the local admin surface.
pub const TOOL_PERMISSION_REQUESTS: &str = "permission_requests";
/// MCP tool: list guided workflows for weaker/blank-context clients.
pub const TOOL_WORKFLOWS_LIST: &str = "workflows_list";
/// MCP tool: return exact MCP-call recipes for one guided workflow.
pub const TOOL_WORKFLOW_INFO: &str = "workflow_info";
/// MCP tool: return app-building recipes for common app kinds.
pub const TOOL_APP_RECIPE: &str = "app_recipe";
/// MCP tool: return a minimal generated app bundle as JSON files.
pub const TOOL_APP_SCAFFOLD: &str = "app_scaffold";
/// MCP tool: start a server-side draft app bundle for weak/locked-down clients.
pub const TOOL_APP_BUILD_START: &str = "app_build_start";
/// MCP tool: put or replace one file in a server-side draft app bundle.
pub const TOOL_APP_BUILD_PUT_FILE: &str = "app_build_put_file";
/// MCP tool: inspect a server-side draft app bundle.
pub const TOOL_APP_BUILD_GET: &str = "app_build_get";
/// MCP tool: list server-side draft app bundles (stall/lost-draftId recovery).
pub const TOOL_APP_BUILD_LIST: &str = "app_build_list";
/// MCP tool: validate a server-side draft app bundle without committing.
pub const TOOL_APP_BUILD_VALIDATE: &str = "app_build_validate";
/// MCP tool: commit a validated server-side draft app bundle through app.add.
pub const TOOL_APP_BUILD_COMMIT: &str = "app_build_commit";
/// MCP tool: discard a server-side draft app bundle.
pub const TOOL_APP_BUILD_DISCARD: &str = "app_build_discard";
/// MCP tool: validate an app bundle path before registration.
pub const TOOL_APP_BUNDLE_VALIDATE: &str = "app_bundle_validate";
/// MCP tool: register an app from inline bundle files through the core app.add command.
pub const TOOL_APP_REGISTER_INLINE: &str = "app_register_inline";
/// MCP tool: register an app bundle through the core app.add command.
pub const TOOL_APP_REGISTER: &str = "app_register";
/// MCP tool: list capability docs.
pub const TOOL_CAPABILITIES_LIST: &str = "capabilities_list";
/// MCP tool: return detailed capability docs for one namespace.
pub const TOOL_CAPABILITY_INFO: &str = "capability_info";
/// MCP tool: run a public capability command through the core dispatcher.
pub const TOOL_CAPABILITY_COMMAND: &str = "capability_command";
/// MCP tool: run a read-only public capability query.
pub const TOOL_CAPABILITY_QUERY: &str = "capability_query";

/// The reserved backend verb an app implements to self-describe: `invoke`ing it
/// (or the `app_actions` tool) returns an [`AppActions`] JSON document. Apps that
/// don't implement it simply fall through to their "unknown verb" handling.
pub const ACTIONS_VERB: &str = "__actions__";

/// Server instructions returned in the MCP `initialize` result. Most clients
/// inject this string into the model's system prompt, so it is the one place
/// guidance is guaranteed to reach models that never read tools or resources
/// carefully. Keep it short, imperative, and contract-first: weak-model evals
/// showed failures come from contract precision (backend shape, manifest shape,
/// invoke argument shape), not from discovery.
pub const MCP_SERVER_INSTRUCTIONS: &str = "\
Terrane builds and runs local apps. Build an app in 4 steps: \
1) app_build_start {id,name,kind,withUi} creates a server-side draft; use kind js_kv_app with withUi:true for interactive/UI apps (calendars, dashboards, forms). \
2) app_build_put_file once per file with the COMPLETE file content. \
3) app_build_validate. 4) app_build_commit with draftId + validationToken. \
Then list_apps, app_actions, and invoke to run verbs.\n\
Backend contract: main.js is ONE plain script. No top-level import/export, no require, no modules. Define one global function handle(input). \
input is an array of strings: input[0] is the verb, input.slice(1) are args. Return a string (use JSON.stringify for structured data). \
Storage is ctx.resource.kv (get/set/rm/scan); wrap kv.get in try/catch because missing keys throw.\n\
Manifest contract: manifest.json is exactly {\"id\":\"my-app\",\"name\":\"My App\",\"runtime\":\"js\",\"backend\":\"main.js\",\"ui\":\"index.html\",\"resources\":[\"kv\"]}. \
ui is a string file path (omit it for backend-only apps), never an object.\n\
UI contract: browser code calls window.terrane.invoke(\"verb\",\"arg1\",\"arg2\") with positional string args and awaits the backend's string reply. Do not pass an args array or an object.\n\
Permissions: a result with structuredContent.type==\"permission_required\" is expected, not failure. NEVER call capability_command with auth.* or grant/approve commands. \
Ask a trusted operator to approve adminUrl or run grantCommands, poll permission_check with requestId, then retry the original call unchanged.\n\
Recovery: every tool result names your next step in nextToolCall. If you lose a draftId, call app_build_list, then validate and commit; only read files validation complains about.\n\
Speed: a new withUi draft is a WORKING shell. Do not read scaffold files back — write your complete main.js immediately, keep style.css, and edit only the REPLACE-marked ui.js functions. Keep the __actions__ verb in main.js so verb discovery works after install.";

/// An MCP tool descriptor: its name, a one-line description, and its input
/// JSON Schema (as a JSON string — the MCP host drops it verbatim into the
/// `tools/list` reply).
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
}

/// A static MCP resource the host advertises. Dynamic capability docs are
/// advertised as templates below because their namespace set comes from core.
pub struct ResourceDef {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
}

/// An MCP resource template for dynamic Terrane docs.
pub struct ResourceTemplateDef {
    pub uri_template: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
}

/// A user-invoked MCP prompt Terrane exposes for guided operations.
pub struct PromptDef {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments_schema: &'static str,
}

/// The tools the MCP host advertises, in the order an agent uses them: list →
/// discover → act. The `invoke` shape mirrors [`InvokeRequest`] plus an `app`.
pub fn mcp_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: TOOL_WORKFLOWS_LIST,
            description: "Start here for blank-context or weaker models. Lists MCP workflows plus chooseByOutcome hints for mapping user goals to recipes, such as KV apps, multi-cap apps, bundle registration, app operation, and safe capability commands.",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_WORKFLOW_INFO,
            description: "Return an executable recipe of tools/call steps for one workflow. The primary app-build route is app_build_start, app_build_put_file, app_build_validate, then app_build_commit. Example tools/call arguments: {\"name\":\"workflow_info\",\"arguments\":{\"name\":\"make_js_kv_app\"}}.",
            input_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Workflow id from workflows_list, e.g. make_js_kv_app, make_js_multicap_app_no_filesystem, or register_app_bundle."}},"required":["name"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_RECIPE,
            description: "Return a concise app-building recipe. This is an orientation tool, not a place to draft the whole app in prose. For most JS apps, call app_build_start, update files one at a time with app_build_put_file, validate, commit, app_actions, then invoke. For visible UI apps, pass withUi:true and call window.terrane.invoke(\"verb\", \"arg1\", \"arg2\") from browser code. For optional KV index keys, use a kvGetOrNull-style helper before JSON.parse.",
            input_schema: r#"{"type":"object","properties":{"kind":{"type":"string","description":"Recipe kind. Defaults to js_kv_app. Use js_multicap_audit for a kv+crdt+relational_db app."}},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_START,
            description: "Step 1 of 4 of the primary app-build flow. Creates a server-side draft from a working template so you edit one file at a time. Use kind js_kv_app with withUi:true for interactive/UI apps such as calendars, dashboards, forms, and natural-language input pages. Returns draftId, file summaries, the exact backend/manifest/UI contract, and nextToolCall. Scaffold files are placeholders: replace their content for the requested app.",
            input_schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"Safe app id, e.g. calendar-demo."},"name":{"type":"string","description":"Display name, e.g. Calendar Demo."},"kind":{"type":"string","enum":["js_kv_app","js_kv_notes","js_multicap_audit"],"description":"js_kv_app for interactive/UI apps over KV (most tasks). js_kv_notes for a minimal backend-only notes demo. js_multicap_audit for a kv+crdt+relational_db proof. Defaults to js_kv_notes."},"withUi":{"type":"boolean","description":"Include index.html, ui.js, and style.css. Use true for any app a person looks at."}},"required":["id","name"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_PUT_FILE,
            description: "Step 2 of 4: replace draft files with COMPLETE content. Send either path+content for one file, or files:[{path,content},...] to write several files in one call. Backend contract: main.js is one plain script (no top-level import/export, no modules); define one global function handle(input) where input is an array of strings and input[0] is the verb; return a string. UI contract: browser code calls window.terrane.invoke(\"verb\",\"arg1\",\"arg2\") with positional string args.",
            input_schema: r#"{"type":"object","properties":{"draftId":{"type":"string","description":"Draft id returned by app_build_start or app_register_inline dryRun."},"path":{"type":"string","description":"Safe relative bundle path such as main.js, index.html, ui.js, or style.css. Use with content for a single file."},"content":{"type":"string","description":"Complete new file content. Use with path."},"files":{"type":"array","description":"Batch mode: several complete files in one call. A real JSON array of {path,content} objects; one bad entry rejects the whole call and writes nothing.","items":{"type":"object","properties":{"path":{"type":"string","description":"Safe relative bundle path."},"content":{"type":"string","description":"Complete file content."}},"required":["path","content"],"additionalProperties":false}}},"required":["draftId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_GET,
            description: "Inspect a server-side draft app bundle after a stall or before validation. Defaults to summaries only; set includeContent:true with path to recover one file. Does not commit or run effects. If you lost the draftId, call app_build_list first.",
            input_schema: r#"{"type":"object","properties":{"draftId":{"type":"string","description":"Draft id returned by app_build_start or app_register_inline dryRun."},"path":{"type":"string","description":"Optional safe relative file path. When omitted, returns all file summaries."},"includeContent":{"type":"boolean","description":"When true and path is set, include that file's content. Defaults to false."}},"required":["draftId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_LIST,
            description: "List server-side draft app bundles: draftId, app id/name, status, and file summaries. Use this to recover a lost draftId after a stall or restart, then continue with app_build_get, app_build_put_file, or app_build_validate.",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_VALIDATE,
            description: "Step 3 of 4: validate the draft and dry-run app.add without writing the owned app or appending events. Returns structured errors with fix-it guidance, warnings, and validationToken. After valid:true, call app_build_commit with draftId and validationToken; do not resend the files.",
            input_schema: r#"{"type":"object","properties":{"draftId":{"type":"string","description":"Draft id returned by app_build_start or app_register_inline dryRun."}},"required":["draftId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_COMMIT,
            description: "Step 4 of 4: commit a validated server-side draft. Revalidates, writes the owned bundle under TERRANE_HOME/apps/<id>, then dispatches app.add through core and deletes the draft. Use validationToken from app_build_validate. Create-only: existing app ids are refused.",
            input_schema: r#"{"type":"object","properties":{"draftId":{"type":"string","description":"Draft id returned by app_build_start or app_register_inline dryRun."},"validationToken":{"type":"string","description":"Token returned by app_build_validate. If provided and the draft changed since validation, commit is refused."}},"required":["draftId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUILD_DISCARD,
            description: "Discard a server-side draft app bundle. Removes draft files only; never touches installed apps or app events.",
            input_schema: r#"{"type":"object","properties":{"draftId":{"type":"string","description":"Draft id returned by app_build_start or app_register_inline dryRun."}},"required":["draftId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_SCAFFOLD,
            description: "Compatibility tool: generate a complete JS app bundle as JSON files without a server-side draft. Prefer app_build_start. Use kind js_kv_notes for KV or js_multicap_audit for a kv+crdt+relational_db app. Set withUi:true for calendars, dashboards, forms, and natural-language input pages. The scaffold demonstrates ui.js separation and defensive KV reads.",
            input_schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"Safe app id, e.g. notes-demo."},"name":{"type":"string","description":"Display name, e.g. Notes Demo."},"kind":{"type":"string","enum":["js_kv_app","js_kv_notes","js_multicap_audit"],"description":"Scaffold kind. Defaults to js_kv_notes."},"withUi":{"type":"boolean","description":"Include index.html and style.css. Defaults to false."}},"required":["id","name"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_BUNDLE_VALIDATE,
            description: "Validate an app bundle path before registering it. Example tools/call arguments: {\"name\":\"app_bundle_validate\",\"arguments\":{\"path\":\"/tmp/my-app\"}}.",
            input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory containing manifest.json and referenced backend/UI files."}},"required":["path"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_REGISTER_INLINE,
            description: "Legacy/advanced inline registration for clients that can pass large JSON arrays reliably. files must be a JSON array of {path,content} objects, not a JSON string. Prefer app_build_start/app_build_put_file/app_build_validate/app_build_commit for weak models. dryRun:true now also stores a server-side draft and returns draftId/validationToken so the client can commit with app_build_commit without resending files. If the id already exists, operate it with app_actions or ask a trusted operator to remove/replace it; untrusted capability_command app.remove is refused.",
            input_schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"Optional id override; defaults to manifest.id."},"name":{"type":"string","description":"Optional display-name override; defaults to manifest.name or id."},"runtime":{"type":"string","description":"Optional runtime override; defaults to manifest.runtime or js."},"files":{"type":"array","description":"Bundle files as a real JSON array, usually structuredContent.files from app_scaffold. Do not JSON-stringify this array. Each item has path and content; retries must include the complete bundle, not only changed files.","items":{"type":"object","properties":{"path":{"type":"string","description":"Safe relative path such as manifest.json or main.js."},"content":{"type":"string","description":"Complete file content."}},"required":["path","content"],"additionalProperties":false}},"dryRun":{"type":"boolean","description":"Validate and dry-run app.add without writing files or committing. Defaults to false."}},"required":["files"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_REGISTER,
            description: "Happy-path app registration for agents. Reads manifest.json from source, validates the bundle, then dispatches app.add through core. Use dryRun true before committing.",
            input_schema: r#"{"type":"object","properties":{"source":{"type":"string","description":"App bundle directory containing manifest.json."},"id":{"type":"string","description":"Optional id override; defaults to manifest.id."},"name":{"type":"string","description":"Optional display-name override; defaults to manifest.name or id."},"runtime":{"type":"string","description":"Optional runtime override; defaults to manifest.runtime or js."},"dryRun":{"type":"boolean","description":"Validate through app.add without committing. Defaults to false."}},"required":["source"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_LIST_APPS,
            description: "List the installed terrane apps (id, name, whether it has a UI). Returns structuredContent.apps as well as text JSON.",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_ACTIONS,
            description: "Describe an app's available actions (verbs and their args), as the app \
                          declares them. Call this before `invoke` to discover what an app can do. \
                          If a requested resource is not yet granted this returns isError:true with a \
                          permission_required object in structuredContent. The model cannot grant this \
                          through MCP: do not call capability_command with auth.* or grant names. Ask a \
                          trusted operator to run grantCommands or approve adminUrl, poll status with \
                          permission_check, then retry the same app_actions call.",
            input_schema: r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_INVOKE,
            description: "Run a verb on an app's backend and return its string output, \
                          e.g. {\"app\":\"todo-cli-collaborate\",\"verb\":\"add\",\"args\":[\"buy milk\"]}. \
                          On an ungranted resource this returns isError:true with a permission_required object \
                          in structuredContent. The model cannot grant this through MCP: do not call \
                          capability_command with auth.* or grant names. Ask a trusted operator to run \
                          grantCommands or approve adminUrl, poll status with permission_check, then retry \
                          the same invoke call.",
            input_schema: r#"{"type":"object","properties":{"app":{"type":"string"},"verb":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["app","verb"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_PERMISSION_CHECK,
            description: "Check a permission request returned by app_actions, invoke, or grant-gated capability_command. This polls status only; it does not grant. Use the requestId from a permission_required response. If status is approved, immediately retry the exact original app_actions/invoke/capability_command call with the same arguments.",
            input_schema: r#"{"type":"object","properties":{"requestId":{"type":"string","description":"Permission request id returned by permission_required."}},"required":["requestId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_PERMISSION_CANCEL,
            description: "Cancel a pending permission request. This does not grant access; approval remains a trusted admin UI action.",
            input_schema: r#"{"type":"object","properties":{"requestId":{"type":"string","description":"Permission request id returned by permission_required."},"reason":{"type":"string","description":"Optional cancellation reason."}},"required":["requestId"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_PERMISSION_REQUESTS,
            description: "List local permission requests and their pending/approved/denied/cancelled status. This does not grant. If a request is approved, immediately retry the exact original app_actions/invoke/capability_command call.",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITIES_LIST,
            description: "List Terrane capability namespaces and short summaries.",
            input_schema: r#"{"type":"object","properties":{"includeInternal":{"type":"boolean","description":"Include internal-only capability notes. Defaults to false."}},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITY_INFO,
            description: "Return detailed Terrane capability documentation for one namespace. For app registration docs, call with {\"namespace\":\"app\",\"format\":\"json\"}.",
            input_schema: r#"{"type":"object","properties":{"namespace":{"type":"string","description":"Capability namespace, e.g. kv, crdt, relational_db."},"format":{"type":"string","enum":["json","markdown","skill"],"description":"Rendered output format. Defaults to json."},"includeInternal":{"type":"boolean","description":"Include internal-only implementation notes. Defaults to false."}},"required":["namespace"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITY_QUERY,
            description: "Run a read-only Terrane capability query, e.g. {\"capability\":\"app\",\"query\":\"exists\",\"args\":[\"todo\"]}.",
            input_schema: r#"{"type":"object","properties":{"capability":{"type":"string","description":"Capability namespace, e.g. app or replica."},"query":{"type":"string","description":"Query name, either local (exists) or dotted (app.exists)."},"args":{"type":"array","items":{"type":"string"},"description":"Query argument vector. Defaults to []."}},"required":["capability","query"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITY_COMMAND,
            description: "Run an allowed Terrane capability command through the core dispatcher. First call tools/call with {\"name\":\"capability_command\",\"arguments\":{\"name\":\"app.add\",\"help\":true}} for ordered params and examples. Set dryRun true to validate simple commit commands without mutation. This tool cannot approve permission_required and refuses auth.*, grant/approve, app.remove, raw storage, runtime, network, model, and harness effect commands on the public MCP path. If a resource command returns permission_required, use permission_check/admin approval, not auth.grant through this tool.",
            input_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Dotted command name, e.g. app.add or kv.set."},"args":{"type":"array","items":{"type":"string"},"description":"Command argument vector in the order returned by help:true / capability docs. Defaults to []."},"dryRun":{"type":"boolean","description":"Validate without committing when the command can be decided locally. Defaults to false."},"help":{"type":"boolean","description":"Return ordered parameter docs, effects, errors, and examples for name without executing. Defaults to false."}},"required":["name"],"additionalProperties":false}"#,
        },
    ]
}

/// Static overall MCP documentation resources owned by `host/mcp`.
pub fn mcp_resources() -> Vec<ResourceDef> {
    vec![
        ResourceDef {
            uri: "terrane://docs/index",
            name: "Terrane MCP Guide",
            description: "Overall MCP manual: connection model, ownership boundaries, and where to start.",
            mime_type: "text/markdown",
        },
        ResourceDef {
            uri: "terrane://docs/clients",
            name: "Terrane MCP Clients",
            description: "Client configuration notes for stdio, HTTP, Claude, opencode, Codex, and generic JSON-RPC.",
            mime_type: "text/markdown",
        },
        ResourceDef {
            uri: "terrane://docs/app-building",
            name: "Terrane MCP App Building",
            description: "App-building workflows, including inline registration for locked-down clients.",
            mime_type: "text/markdown",
        },
        ResourceDef {
            uri: "terrane://docs/capability-operations",
            name: "Terrane MCP Capability Operations",
            description: "How to use capability docs, read-only queries, and guarded command dispatch.",
            mime_type: "text/markdown",
        },
        ResourceDef {
            uri: "terrane://docs/security",
            name: "Terrane MCP Security",
            description: "Local MCP security, permissions, approval, destructive actions, and logging guidance.",
            mime_type: "text/markdown",
        },
        ResourceDef {
            uri: "terrane://docs/agent-playbook",
            name: "Terrane MCP Agent Playbook",
            description: "Agent playbook for no-source, no-shell app creation and the permission grant handshake.",
            mime_type: "text/markdown",
        },
    ]
}

/// Dynamic MCP resource templates. The concrete content is served by the host
/// because it has access to core capability docs.
pub fn mcp_resource_templates() -> Vec<ResourceTemplateDef> {
    vec![
        ResourceTemplateDef {
            uri_template: "terrane://capabilities/{namespace}",
            name: "Terrane Capability Doc",
            description:
                "Capability-owned docs from terrane-cap-*/src/doc.rs rendered as markdown.",
            mime_type: "text/markdown",
        },
        ResourceTemplateDef {
            uri_template: "terrane://workflows/{name}",
            name: "Terrane MCP Workflow",
            description: "Executable workflow recipe from the host MCP layer.",
            mime_type: "application/json",
        },
    ]
}

/// User-invoked prompt recipes for MCP clients that support prompts.
pub fn mcp_prompts() -> Vec<PromptDef> {
    vec![
        PromptDef {
            name: "make_js_kv_app",
            description: "Create, register, inspect, and invoke a small JS app backed by kv.",
            arguments_schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"App id, e.g. notes-demo."},"name":{"type":"string","description":"Display name, e.g. Notes Demo."},"text":{"type":"string","description":"Initial note text to write after registration."}},"additionalProperties":false}"#,
        },
        PromptDef {
            name: "register_app_bundle",
            description: "Validate and register an existing app bundle directory.",
            arguments_schema: r#"{"type":"object","properties":{"source":{"type":"string","description":"Bundle directory containing manifest.json."}},"required":["source"],"additionalProperties":false}"#,
        },
        PromptDef {
            name: "inspect_app_actions",
            description: "List apps and inspect one app's self-declared actions.",
            arguments_schema: r#"{"type":"object","properties":{"app":{"type":"string","description":"App id to inspect."}},"additionalProperties":false}"#,
        },
        PromptDef {
            name: "safe_capability_command",
            description:
                "Use capability_command help and dryRun before dispatching a low-level command.",
            arguments_schema: r#"{"type":"object","properties":{"command":{"type":"string","description":"Dotted command name, e.g. app.add or kv.set."}},"additionalProperties":false}"#,
        },
    ]
}

/// An app's self-description, returned by its [`ACTIONS_VERB`] backend verb and
/// surfaced by the MCP `app_actions` tool. Apps emit this as JSON; clients parse
/// it to drive the app programmatically.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct AppActions {
    pub app: String,
    #[nserde(default)]
    pub title: String,
    #[nserde(default)]
    pub description: String,
    pub actions: Vec<Action>,
}

/// One action an app exposes.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct Action {
    /// The verb to pass to `invoke` (the first element of `args`).
    pub verb: String,
    #[nserde(default)]
    pub summary: String,
    #[nserde(default)]
    pub args: Vec<ActionArg>,
    #[nserde(default)]
    pub returns: String,
}

/// One positional argument of an [`Action`].
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct ActionArg {
    pub name: String,
    #[nserde(default)]
    pub required: bool,
    #[nserde(default)]
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Exportable contract summary (folded into public-contract.json)
// ---------------------------------------------------------------------------

/// One HTTP route in the exported contract.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct HttpRoute {
    pub method: String,
    pub path: String,
    pub summary: String,
}

/// One MCP tool in the exported contract.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct McpToolEntry {
    pub name: String,
    pub description: String,
}

/// One static MCP resource in the exported contract.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct McpResourceEntry {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// One MCP resource template in the exported contract.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct McpResourceTemplateEntry {
    pub uri_template: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// One MCP prompt in the exported contract.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct McpPromptEntry {
    pub name: String,
    pub description: String,
}

/// The host-API slice of `public-contract.json`: the routes and tools terrane's
/// hosts serve and premium pins. The `terrane contract export` step serializes
/// this (alongside the capability surface from `terrane-core`).
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct HostContract {
    pub contract_version: String,
    pub mcp_protocol_version: String,
    pub http_routes: Vec<HttpRoute>,
    pub mcp_tools: Vec<McpToolEntry>,
    #[nserde(default)]
    pub mcp_resources: Vec<McpResourceEntry>,
    #[nserde(default)]
    pub mcp_resource_templates: Vec<McpResourceTemplateEntry>,
    #[nserde(default)]
    pub mcp_prompts: Vec<McpPromptEntry>,
}

// ---------------------------------------------------------------------------
// Public surface (the Rust-introspectable core of public-contract.json)
// ---------------------------------------------------------------------------

/// One method of a capability's backend `ctx.resource` surface.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct ResourceMethodInfo {
    pub name: String,
    /// `"read"` or `"write"`.
    pub kind: String,
    pub params: Vec<String>,
}

/// A capability's declared `ctx.resource.<namespace>` surface.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct ResourceNamespace {
    pub namespace: String,
    pub methods: Vec<ResourceMethodInfo>,
    #[nserde(default)]
    pub grant_specs: Vec<GrantResourceSpecInfo>,
}

/// Machine-readable grant metadata for one resource namespace selector schema.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct GrantResourceSpecInfo {
    pub namespace: String,
    #[nserde(rename = "selectorSchemaId")]
    pub selector_schema_id: String,
    #[nserde(rename = "selectorSchemaJson")]
    pub selector_schema_json: String,
    pub verbs: Vec<String>,
    pub compatibility: GrantResourceCompatibilityInfo,
    #[nserde(rename = "unknownSelectorSchemaPolicy")]
    pub unknown_selector_schema_policy: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, SerJson, DeJson)]
pub struct GrantResourceCompatibilityInfo {
    pub backward: bool,
    pub forward: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityList {
    pub capabilities: Vec<CapabilitySummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilitySummary {
    pub namespace: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub resources: Vec<String>,
    pub commands: Vec<String>,
    #[nserde(default)]
    pub queries: Vec<String>,
    pub events: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityDocInfo {
    pub namespace: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub version: String,
    pub audience: Vec<String>,
    pub manifest: CapabilityManifestInfo,
    #[nserde(default)]
    pub commands: Vec<CapabilityCommandInfo>,
    #[nserde(default)]
    pub queries: Vec<CapabilityQueryInfo>,
    #[nserde(default)]
    pub events: Vec<CapabilityEventInfo>,
    pub resources: Vec<CapabilityResourceInfo>,
    pub schemas: Vec<CapabilitySchemaInfo>,
    pub examples: Vec<CapabilityExampleInfo>,
    pub constraints: Vec<String>,
    pub limits: Vec<CapabilityLimitInfo>,
    pub compatibility: Vec<String>,
    #[nserde(default)]
    pub internal: Vec<CapabilityInternalInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityManifestInfo {
    pub commands: Vec<String>,
    pub queries: Vec<String>,
    pub events: Vec<String>,
    pub subscriptions: Vec<String>,
    pub resource_methods: Vec<CapabilityResourceMethodInfo>,
    #[nserde(default)]
    pub grant_resources: Vec<GrantResourceSpecInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityCommandInfo {
    pub name: String,
    #[nserde(default)]
    pub summary: String,
    #[nserde(default)]
    pub params: Vec<CapabilityParamInfo>,
    #[nserde(default)]
    pub returns: String,
    #[nserde(default)]
    pub errors: Vec<String>,
    #[nserde(default)]
    pub emits: Vec<String>,
    #[nserde(default)]
    pub effects: Vec<String>,
    #[nserde(default)]
    pub examples: Vec<CapabilityExampleInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityCommandHelpInfo {
    pub name: String,
    #[nserde(default)]
    pub summary: String,
    #[nserde(default)]
    pub argument_order: Vec<String>,
    #[nserde(default)]
    pub params: Vec<CapabilityParamInfo>,
    #[nserde(default)]
    pub returns: String,
    #[nserde(default)]
    pub errors: Vec<String>,
    #[nserde(default)]
    pub emits: Vec<String>,
    #[nserde(default)]
    pub effects: Vec<String>,
    #[nserde(default)]
    pub examples: Vec<CapabilityExampleInfo>,
    #[nserde(default)]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityQueryInfo {
    pub name: String,
    #[nserde(default)]
    pub summary: String,
    #[nserde(default)]
    pub params: Vec<CapabilityParamInfo>,
    #[nserde(default)]
    pub returns: String,
    #[nserde(default)]
    pub errors: Vec<String>,
    #[nserde(default)]
    pub examples: Vec<CapabilityExampleInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityEventInfo {
    pub kind: String,
    #[nserde(default)]
    pub summary: String,
    #[nserde(default)]
    pub params: Vec<CapabilityParamInfo>,
    #[nserde(default)]
    pub effects: Vec<String>,
    #[nserde(default)]
    pub examples: Vec<CapabilityExampleInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityResourceInfo {
    pub namespace: String,
    pub summary: String,
    pub methods: Vec<CapabilityResourceMethodInfo>,
    #[nserde(default)]
    pub grant_specs: Vec<GrantResourceSpecInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityResourceMethodInfo {
    pub name: String,
    pub kind: String,
    pub params: Vec<CapabilityParamInfo>,
    pub returns: String,
    pub summary: String,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityParamInfo {
    pub name: String,
    pub summary: String,
    pub required: bool,
    pub schema_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilitySchemaInfo {
    pub id: String,
    pub title: String,
    pub media_type: String,
    pub schema_json: String,
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityExampleInfo {
    pub title: String,
    pub summary: String,
    pub language: String,
    pub code: String,
    pub expected: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityLimitInfo {
    pub name: String,
    pub value: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityInternalInfo {
    pub title: String,
    pub body: String,
}

/// How a client runs an app, and how an app self-describes.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct AppContractInfo {
    /// The reserved backend verb an app implements to self-describe ([`ACTIONS_VERB`]).
    pub actions_verb: String,
    /// The invoke contract (verb + string args → string).
    pub invoke: String,
}

/// The sync wire surface — what flows between replicas, and over what.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct SyncInfo {
    /// The recorded event that IS the wire format.
    pub wire_event: String,
    pub syncable_event_kinds: Vec<String>,
    pub transports: Vec<String>,
}

/// The Rust-introspectable public surface — the authoritative core of the
/// exported `public-contract.json` that `terrane-premium` pins (the export tool
/// wraps this with provenance, license, conformance commands, and file hashes).
/// Everything here is derived from declarations, so it can't drift from the code.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct PublicSurface {
    pub contract_version: String,
    pub host: HostContract,
    pub capabilities: Vec<String>,
    pub resources: Vec<ResourceNamespace>,
    pub capability_docs: Vec<CapabilityDocInfo>,
    pub app: AppContractInfo,
    pub sync: SyncInfo,
}

/// Assemble the public surface from the host contract plus the capability and
/// resource surfaces (which the caller reads from `terrane-core`, since this
/// crate stays free of that dependency).
pub fn public_surface(
    capabilities: Vec<String>,
    resources: Vec<ResourceNamespace>,
    capability_docs: Vec<CapabilityDocInfo>,
) -> PublicSurface {
    PublicSurface {
        contract_version: CONTRACT_VERSION.to_string(),
        host: host_contract(),
        capabilities,
        resources,
        capability_docs,
        app: AppContractInfo {
            actions_verb: ACTIONS_VERB.to_string(),
            invoke:
                "a verb plus its string args runs the app backend and returns a string \
                     (HTTP POST /apps/{id}/invoke; MCP `invoke` tool; runtime selected by manifest)"
                    .to_string(),
        },
        sync: SyncInfo {
            wire_event: "crdt.update".to_string(),
            syncable_event_kinds: vec!["crdt.update".to_string()],
            transports: vec![
                "file: terrane sync <app> --from <home>".to_string(),
                "tcp: terrane serve / terrane sync <app> --peer <addr>".to_string(),
            ],
        },
    }
}

/// Build the canonical host-API contract summary from the declarations above —
/// the single place the exported artifact is derived from.
pub fn host_contract() -> HostContract {
    let route = |method: &str, path: &str, summary: &str| HttpRoute {
        method: method.to_string(),
        path: path.to_string(),
        summary: summary.to_string(),
    };
    HostContract {
        contract_version: CONTRACT_VERSION.to_string(),
        mcp_protocol_version: MCP_PROTOCOL_VERSION.to_string(),
        http_routes: vec![
            route("GET", ROUTE_HEALTHZ, "Liveness check."),
            route("GET", ROUTE_APPS, "List installed apps."),
            route(
                "POST",
                ROUTE_MCP,
                "MCP JSON-RPC transport over HTTP for the shared host tools.",
            ),
            route("GET", "/apps/{id}/", "Serve an app's UI and assets."),
            route(
                "POST",
                "/apps/{id}/invoke",
                "Run a verb on an app's backend.",
            ),
        ],
        mcp_tools: mcp_tools()
            .into_iter()
            .map(|t| McpToolEntry {
                name: t.name.to_string(),
                description: t.description.to_string(),
            })
            .collect(),
        mcp_resources: mcp_resources()
            .into_iter()
            .map(|r| McpResourceEntry {
                uri: r.uri.to_string(),
                name: r.name.to_string(),
                description: r.description.to_string(),
                mime_type: r.mime_type.to_string(),
            })
            .collect(),
        mcp_resource_templates: mcp_resource_templates()
            .into_iter()
            .map(|r| McpResourceTemplateEntry {
                uri_template: r.uri_template.to_string(),
                name: r.name.to_string(),
                description: r.description.to_string(),
                mime_type: r.mime_type.to_string(),
            })
            .collect(),
        mcp_prompts: mcp_prompts()
            .into_iter()
            .map(|p| McpPromptEntry {
                name: p.name.to_string(),
                description: p.description.to_string(),
            })
            .collect(),
    }
}
