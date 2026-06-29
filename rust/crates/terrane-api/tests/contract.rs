//! Contract tests for `terrane-api`: the wire types round-trip through JSON and
//! the exported surface stays the documented subset.

use nanoserde::{DeJson, SerJson};
use terrane_api::{
    host_contract, mcp_tools, Action, ApiError, AppActions, AppSummary, AppsResponse,
    CapabilityCommandHelpInfo, CapabilityCommandInfo, CapabilityDocInfo, CapabilityEventInfo,
    CapabilityExampleInfo, CapabilityManifestInfo, CapabilityParamInfo, CapabilityQueryInfo,
    HealthResponse, InvokeRequest, InvokeResponse, TOOL_APP_ACTIONS, TOOL_APP_BUNDLE_VALIDATE,
    TOOL_APP_RECIPE, TOOL_APP_REGISTER, TOOL_APP_SCAFFOLD, TOOL_CAPABILITIES_LIST,
    TOOL_CAPABILITY_COMMAND, TOOL_CAPABILITY_INFO, TOOL_CAPABILITY_QUERY, TOOL_INVOKE,
    TOOL_LIST_APPS, TOOL_WORKFLOWS_LIST, TOOL_WORKFLOW_INFO,
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
    let health = HealthResponse {
        status: "ok".into(),
        version: "0.1.0".into(),
    };
    assert_eq!(
        HealthResponse::deserialize_json(&health.serialize_json()).unwrap(),
        health
    );

    let apps = AppsResponse {
        apps: vec![AppSummary {
            id: "todo".into(),
            name: "Todo".into(),
            has_ui: true,
        }],
    };
    assert_eq!(
        AppsResponse::deserialize_json(&apps.serialize_json()).unwrap(),
        apps
    );

    let out = InvokeResponse {
        output: "added: buy milk".into(),
    };
    assert_eq!(
        InvokeResponse::deserialize_json(&out.serialize_json()).unwrap(),
        out
    );

    let err = ApiError {
        error: "no such app".into(),
    };
    assert_eq!(
        ApiError::deserialize_json(&err.serialize_json()).unwrap(),
        err
    );
}

#[test]
fn mcp_tool_surface_is_the_documented_set_with_valid_schemas() {
    let tools = mcp_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
            TOOL_WORKFLOWS_LIST,
            TOOL_WORKFLOW_INFO,
            TOOL_APP_RECIPE,
            TOOL_APP_SCAFFOLD,
            TOOL_APP_BUNDLE_VALIDATE,
            TOOL_APP_REGISTER,
            TOOL_LIST_APPS,
            TOOL_APP_ACTIONS,
            TOOL_INVOKE,
            TOOL_CAPABILITIES_LIST,
            TOOL_CAPABILITY_INFO,
            TOOL_CAPABILITY_QUERY,
            TOOL_CAPABILITY_COMMAND
        ]
    );

    // Each tool's input schema must parse as a JSON object — it's dropped verbatim
    // into the MCP tools/list reply, so a malformed one would break the protocol.
    for tool in &tools {
        SchemaProbe::deserialize_json(tool.input_schema)
            .unwrap_or_else(|e| panic!("tool {} has a malformed input schema: {e}", tool.name));
    }

    let command_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_CAPABILITY_COMMAND)
        .expect("capability_command tool exists");
    assert!(
        command_tool.description.contains("help")
            && command_tool.description.contains("ordered params")
            && command_tool.input_schema.contains(r#""help""#),
        "capability_command should advertise first-hop command help: {} / {}",
        command_tool.description,
        command_tool.input_schema
    );

    let workflow_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_WORKFLOW_INFO)
        .expect("workflow_info tool exists");
    assert!(
        workflow_tool.description.contains("executable recipe")
            && workflow_tool.input_schema.contains("make_js_kv_app"),
        "workflow_info should advertise weak-model recipes: {} / {}",
        workflow_tool.description,
        workflow_tool.input_schema
    );
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

    let routes: Vec<(&str, &str)> = c
        .http_routes
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    assert_eq!(
        routes,
        vec![
            ("GET", "/healthz"),
            ("GET", "/apps"),
            ("POST", "/mcp"),
            ("GET", "/apps/{id}/"),
            ("POST", "/apps/{id}/invoke"),
        ]
    );

    let tool_names: Vec<&str> = c.mcp_tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        tool_names,
        vec![
            TOOL_WORKFLOWS_LIST,
            TOOL_WORKFLOW_INFO,
            TOOL_APP_RECIPE,
            TOOL_APP_SCAFFOLD,
            TOOL_APP_BUNDLE_VALIDATE,
            TOOL_APP_REGISTER,
            TOOL_LIST_APPS,
            TOOL_APP_ACTIONS,
            TOOL_INVOKE,
            TOOL_CAPABILITIES_LIST,
            TOOL_CAPABILITY_INFO,
            TOOL_CAPABILITY_QUERY,
            TOOL_CAPABILITY_COMMAND
        ]
    );

    // The whole contract serializes (this is what the export folds in).
    assert!(c.serialize_json().contains("\"contract_version\""));
}

#[test]
fn capability_doc_info_round_trips() {
    let doc = CapabilityDocInfo {
        namespace: "kv".into(),
        title: "kv".into(),
        summary: "Key/value storage".into(),
        status: "stable".into(),
        version: "0.1.0".into(),
        audience: vec!["agent".into()],
        manifest: CapabilityManifestInfo {
            commands: vec!["kv.set".into()],
            queries: vec![],
            events: vec!["kv.set".into()],
            subscriptions: vec![],
            resource_methods: vec![],
        },
        commands: vec![CapabilityCommandInfo {
            name: "kv.set".into(),
            summary: "Store one key.".into(),
            params: vec![
                CapabilityParamInfo {
                    name: "key".into(),
                    summary: "Key to store.".into(),
                    required: true,
                    schema_ref: "kv.key".into(),
                },
                CapabilityParamInfo {
                    name: "value".into(),
                    summary: "Value to store.".into(),
                    required: true,
                    schema_ref: "string".into(),
                },
            ],
            returns: "Decision".into(),
            errors: vec!["invalid input".into()],
            emits: vec!["kv.set".into()],
            effects: vec!["updates folded kv state".into()],
            examples: vec![CapabilityExampleInfo {
                title: "Set key".into(),
                summary: "Stores a value.".into(),
                language: "text".into(),
                code: "kv.set greeting hello".into(),
                expected: "kv.set event".into(),
            }],
        }],
        queries: vec![CapabilityQueryInfo {
            name: "kv.exists".into(),
            summary: "Check key presence.".into(),
            params: vec![CapabilityParamInfo {
                name: "key".into(),
                summary: "Key to check.".into(),
                required: true,
                schema_ref: "kv.key".into(),
            }],
            returns: "bool".into(),
            errors: vec![],
            examples: vec![],
        }],
        events: vec![CapabilityEventInfo {
            kind: "kv.set".into(),
            summary: "A key was stored.".into(),
            params: vec![
                CapabilityParamInfo {
                    name: "key".into(),
                    summary: "Stored key.".into(),
                    required: true,
                    schema_ref: "kv.key".into(),
                },
                CapabilityParamInfo {
                    name: "value".into(),
                    summary: "Stored value.".into(),
                    required: true,
                    schema_ref: "string".into(),
                },
            ],
            effects: vec!["folds into the kv projection".into()],
            examples: vec![],
        }],
        resources: vec![],
        schemas: vec![],
        examples: vec![],
        constraints: vec![],
        limits: vec![],
        compatibility: vec![],
        internal: vec![],
    };
    assert_eq!(
        CapabilityDocInfo::deserialize_json(&doc.serialize_json()).unwrap(),
        doc
    );
}

#[test]
fn capability_command_help_info_round_trips() {
    let help = CapabilityCommandHelpInfo {
        name: "app.add".into(),
        summary: "Record a saved app catalog entry.".into(),
        argument_order: vec![
            "id".into(),
            "name".into(),
            "source".into(),
            "runtime".into(),
        ],
        params: vec![CapabilityParamInfo {
            name: "id".into(),
            summary: "Stable app id.".into(),
            required: true,
            schema_ref: "app_id".into(),
        }],
        returns: "commit".into(),
        errors: vec!["duplicate app".into()],
        emits: vec!["app.added".into()],
        effects: vec![],
        examples: vec![CapabilityExampleInfo {
            title: "Dry-run add".into(),
            summary: "Validate before committing.".into(),
            language: "json".into(),
            code: r#"{"name":"app.add","args":["demo","Demo"],"dryRun":true}"#.into(),
            expected: r#"{"dryRun":true,"records":1}"#.into(),
        }],
        notes: vec!["help:true never dispatches.".into()],
    };
    assert_eq!(
        CapabilityCommandHelpInfo::deserialize_json(&help.serialize_json()).unwrap(),
        help
    );
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
    assert_eq!(
        parsed.actions[1],
        Action {
            verb: "list".into(),
            summary: String::new(),
            args: vec![],
            returns: String::new()
        }
    );
    assert_eq!(
        AppActions::deserialize_json(&parsed.serialize_json()).unwrap(),
        parsed
    );
}
