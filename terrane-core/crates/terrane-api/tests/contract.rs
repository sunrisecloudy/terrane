//! Contract tests for `terrane-api`: the wire types round-trip through JSON and
//! the exported surface stays the documented subset.

use nanoserde::{DeJson, SerJson};
use terrane_api::{
    host_contract, mcp_tools, Action, AppActions, AppSummary, AppsResponse, ApiError,
    HealthResponse, InvokeRequest, InvokeResponse, TOOL_APP_ACTIONS, TOOL_INVOKE, TOOL_LIST_APPS,
};

#[test]
fn invoke_request_round_trips() {
    let req = InvokeRequest {
        verb: "add".into(),
        args: vec!["buy milk".into(), "x".into()],
    };
    let json = req.serialize_json();
    assert_eq!(InvokeRequest::deserialize_json(&json).unwrap(), req);
}

#[test]
fn invoke_request_parses_real_client_json() {
    // Exactly what the web shim / MCP `invoke` tool sends.
    let req = InvokeRequest::deserialize_json(r#"{"verb":"add","args":["buy milk"]}"#).unwrap();
    assert_eq!(req.verb, "add");
    assert_eq!(req.args, vec!["buy milk".to_string()]);

    // `args` is optional (matches the MCP invoke schema) — an arg-less verb may
    // omit it, defaulting to empty.
    let bare = InvokeRequest::deserialize_json(r#"{"verb":"list"}"#).unwrap();
    assert_eq!(bare.verb, "list");
    assert!(bare.args.is_empty());
}

#[test]
fn responses_round_trip() {
    let health = HealthResponse { status: "ok".into(), version: "0.1.0".into() };
    assert_eq!(
        HealthResponse::deserialize_json(&health.serialize_json()).unwrap(),
        health
    );

    let apps = AppsResponse {
        apps: vec![AppSummary { id: "todo".into(), name: "Todo".into(), has_ui: true }],
    };
    assert_eq!(AppsResponse::deserialize_json(&apps.serialize_json()).unwrap(), apps);

    let out = InvokeResponse { output: "added: buy milk".into() };
    assert_eq!(InvokeResponse::deserialize_json(&out.serialize_json()).unwrap(), out);

    let err = ApiError { error: "no such app".into() };
    assert_eq!(ApiError::deserialize_json(&err.serialize_json()).unwrap(), err);
}

#[test]
fn mcp_tool_surface_is_the_documented_set_with_valid_schemas() {
    let tools = mcp_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
    assert_eq!(names, vec![TOOL_LIST_APPS, TOOL_APP_ACTIONS, TOOL_INVOKE]);

    // Each tool's input schema must parse as a JSON object — it's dropped verbatim
    // into the MCP tools/list reply, so a malformed one would break the protocol.
    for tool in &tools {
        SchemaProbe::deserialize_json(tool.input_schema)
            .unwrap_or_else(|e| panic!("tool {} has a malformed input schema: {e}", tool.name));
    }
}

/// A permissive probe — we only need each schema string to parse as a JSON
/// object with a `type`, not to bind its full shape.
#[derive(DeJson)]
struct SchemaProbe {
    #[nserde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
}

#[test]
fn host_contract_lists_the_v1_subset() {
    let c = host_contract();
    assert_eq!(c.contract_version, terrane_api::CONTRACT_VERSION);

    let routes: Vec<(&str, &str)> =
        c.http_routes.iter().map(|r| (r.method.as_str(), r.path.as_str())).collect();
    assert_eq!(
        routes,
        vec![
            ("GET", "/healthz"),
            ("GET", "/apps"),
            ("GET", "/apps/{id}/"),
            ("POST", "/apps/{id}/invoke"),
        ]
    );

    let tool_names: Vec<&str> = c.mcp_tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tool_names, vec![TOOL_LIST_APPS, TOOL_APP_ACTIONS, TOOL_INVOKE]);

    // The whole contract serializes (this is what the export folds in).
    assert!(c.serialize_json().contains("\"contract_version\""));
}

#[test]
fn app_actions_document_round_trips() {
    // The shape an app's `__actions__` verb emits, which `app_actions` surfaces.
    let json = r#"{"app":"todo","title":"Todo","description":"d","actions":[{"verb":"add","summary":"Add","args":[{"name":"text","required":true,"summary":"the text"}],"returns":"ok"},{"verb":"list","args":[]}]}"#;
    let parsed = AppActions::deserialize_json(json).unwrap();
    assert_eq!(parsed.app, "todo");
    assert_eq!(parsed.actions.len(), 2);
    assert_eq!(parsed.actions[0].verb, "add");
    assert!(parsed.actions[0].args[0].required);
    // `list` omits the optional fields and still parses.
    assert_eq!(parsed.actions[1], Action { verb: "list".into(), summary: String::new(), args: vec![], returns: String::new() });
    assert_eq!(AppActions::deserialize_json(&parsed.serialize_json()).unwrap(), parsed);
}
