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
pub const CONTRACT_VERSION: &str = "0.2.0";

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
/// MCP tool: list capability docs.
pub const TOOL_CAPABILITIES_LIST: &str = "capabilities_list";
/// MCP tool: return detailed capability docs for one namespace.
pub const TOOL_CAPABILITY_INFO: &str = "capability_info";

/// The reserved backend verb an app implements to self-describe: `invoke`ing it
/// (or the `app_actions` tool) returns an [`AppActions`] JSON document. Apps that
/// don't implement it simply fall through to their "unknown verb" handling.
pub const ACTIONS_VERB: &str = "__actions__";

/// An MCP tool descriptor: its name, a one-line description, and its input
/// JSON Schema (as a JSON string — the MCP host drops it verbatim into the
/// `tools/list` reply).
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
}

/// The tools the MCP host advertises, in the order an agent uses them: list →
/// discover → act. The `invoke` shape mirrors [`InvokeRequest`] plus an `app`.
pub fn mcp_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: TOOL_LIST_APPS,
            description: "List the installed terrane apps (id, name, whether it has a UI).",
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_APP_ACTIONS,
            description: "Describe an app's available actions (verbs and their args), as the app \
                          declares them. Call this before `invoke` to discover what an app can do.",
            input_schema: r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_INVOKE,
            description: "Run a verb on an app's backend and return its string output, \
                          e.g. {\"app\":\"todo-cli-collaborate\",\"verb\":\"add\",\"args\":[\"buy milk\"]}.",
            input_schema: r#"{"type":"object","properties":{"app":{"type":"string"},"verb":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["app","verb"],"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITIES_LIST,
            description: "List Terrane capability namespaces and short summaries.",
            input_schema: r#"{"type":"object","properties":{"includeInternal":{"type":"boolean","description":"Include internal-only capability notes. Defaults to false."}},"additionalProperties":false}"#,
        },
        ToolDef {
            name: TOOL_CAPABILITY_INFO,
            description: "Return detailed Terrane capability documentation for one namespace.",
            input_schema: r#"{"type":"object","properties":{"namespace":{"type":"string","description":"Capability namespace, e.g. kv, crdt, relational_db."},"format":{"type":"string","enum":["json","markdown","skill"],"description":"Rendered output format. Defaults to json."},"includeInternal":{"type":"boolean","description":"Include internal-only implementation notes. Defaults to false."}},"required":["namespace"],"additionalProperties":false}"#,
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
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct CapabilityResourceInfo {
    pub namespace: String,
    pub summary: String,
    pub methods: Vec<CapabilityResourceMethodInfo>,
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
    }
}
