//! terrane-api — the host API contract.
//!
//! The single source of the surface that terrane's edge hosts expose: the **web
//! host** (HTTP) and the **MCP host** (stdio JSON-RPC). It is the OSS-side typed
//! implementation of the contract that `terrane-premium` consumes as a pinned
//! `public-contract.json` (premium is a *superset* — every route/tool here must
//! exist there too). Kept dependency-light (just nanoserde) so it stays a clean,
//! vendorable contract.
//!
//! What lives here: the wire types (request/response JSON), the route table, the
//! MCP tool descriptors, and [`host_contract`] — the serializable summary that
//! the `terrane contract export` step folds into `public-contract.json`.
//!
//! What does NOT live here: any I/O, any HTTP/MCP server, any dependency on
//! `terrane-core`. The hosts implement this; the core knows nothing of it.

use nanoserde::{DeJson, SerJson};

/// Version of *this* host API surface. Bumped when a route/tool/shape changes.
pub const CONTRACT_VERSION: &str = "0.1.0";

/// The MCP protocol revision the MCP host speaks in its `initialize` handshake.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

// ---------------------------------------------------------------------------
// HTTP routes (web host)
// ---------------------------------------------------------------------------

/// `GET` — liveness. Returns [`HealthResponse`].
pub const ROUTE_HEALTHZ: &str = "/healthz";
/// `GET` — the installed app catalog. Returns [`AppsResponse`].
pub const ROUTE_APPS: &str = "/apps";

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
/// the app's backend (`host.run`).
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
/// MCP tool: run a verb on an app (so an agent can *act* on it).
pub const TOOL_INVOKE: &str = "invoke";

/// An MCP tool descriptor: its name, a one-line description, and its input
/// JSON Schema (as a JSON string — the MCP host drops it verbatim into the
/// `tools/list` reply).
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
}

/// The tools the MCP host advertises. Their `invoke` shape mirrors
/// [`InvokeRequest`] plus an `app` selector.
pub fn mcp_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: TOOL_LIST_APPS,
            description: "List the installed terrane apps (id, name, whether it has a UI).",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_INVOKE,
            description: "Run a verb on an app's backend and return its string output, \
                          e.g. {\"app\":\"todo-cli-collaborate\",\"verb\":\"add\",\"args\":[\"buy milk\"]}.",
            input_schema: r#"{"type":"object","properties":{"app":{"type":"string"},"verb":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["app","verb"],"additionalProperties":false}"#,
        },
    ]
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

/// The host-API slice of `public-contract.json`: the routes and tools premium
/// must implement as a superset. The `terrane contract export` step serializes
/// this (alongside the capability surface from `terrane-core`).
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct HostContract {
    pub contract_version: String,
    pub mcp_protocol_version: String,
    pub http_routes: Vec<HttpRoute>,
    pub mcp_tools: Vec<McpToolEntry>,
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
            route("GET", "/apps/{id}/", "Serve an app's UI and assets."),
            route("POST", "/apps/{id}/invoke", "Run a verb on an app's backend."),
        ],
        mcp_tools: mcp_tools()
            .into_iter()
            .map(|t| McpToolEntry {
                name: t.name.to_string(),
                description: t.description.to_string(),
            })
            .collect(),
    }
}
