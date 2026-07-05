//! Contract tests for `terrane-api`: the wire types round-trip through JSON and
//! the exported surface stays the documented subset.

use nanoserde::{DeJson, SerJson};
use terrane_api::{
    host_contract, mcp_tools, Action, ApiError, AppActions, AppSummary, AppsResponse,
    CapabilityCommandHelpInfo, CapabilityCommandInfo, CapabilityDocInfo, CapabilityEventInfo,
    CapabilityExampleInfo, CapabilityManifestInfo, CapabilityParamInfo, CapabilityQueryInfo,
    HealthResponse, InvokeRequest, InvokeResponse, MCP_SERVER_INSTRUCTIONS, TOOL_APP_ACTIONS,
    TOOL_APP_BUILD_COMMIT, TOOL_APP_BUILD_DISCARD, TOOL_APP_BUILD_GET, TOOL_APP_BUILD_LIST,
    TOOL_APP_BUILD_PUT_FILE, TOOL_APP_BUILD_START, TOOL_APP_BUILD_VALIDATE,
    TOOL_APP_BUNDLE_VALIDATE, TOOL_APP_RECIPE, TOOL_APP_REGISTER, TOOL_APP_REGISTER_INLINE,
    TOOL_APP_SCAFFOLD, TOOL_APP_UPGRADE, TOOL_CAPABILITIES_LIST, TOOL_CAPABILITY_COMMAND,
    TOOL_CAPABILITY_INFO, TOOL_CAPABILITY_QUERY, TOOL_APP_LOGS, TOOL_INVOKE, TOOL_LIST_APPS,
    TOOL_PERMISSION_CANCEL, TOOL_PERMISSION_CHECK, TOOL_PERMISSION_REQUESTS, TOOL_WORKFLOWS_LIST,
    TOOL_WORKFLOW_INFO,
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
            icon: "icon.svg".into(),
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
            TOOL_APP_BUILD_START,
            TOOL_APP_BUILD_PUT_FILE,
            TOOL_APP_BUILD_GET,
            TOOL_APP_BUILD_LIST,
            TOOL_APP_BUILD_VALIDATE,
            TOOL_APP_BUILD_COMMIT,
            TOOL_APP_BUILD_DISCARD,
            TOOL_APP_SCAFFOLD,
            TOOL_APP_BUNDLE_VALIDATE,
            TOOL_APP_REGISTER_INLINE,
            TOOL_APP_REGISTER,
            TOOL_APP_UPGRADE,
            TOOL_LIST_APPS,
            TOOL_APP_ACTIONS,
            TOOL_INVOKE,
            TOOL_APP_LOGS,
            TOOL_PERMISSION_CHECK,
            TOOL_PERMISSION_CANCEL,
            TOOL_PERMISSION_REQUESTS,
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
            && workflow_tool.input_schema.contains("make_js_kv_app")
            && workflow_tool
                .input_schema
                .contains("make_js_multicap_app_no_filesystem"),
        "workflow_info should advertise weak-model recipes: {} / {}",
        workflow_tool.description,
        workflow_tool.input_schema
    );

    let workflows_list_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_WORKFLOWS_LIST)
        .expect("workflows_list tool exists");
    assert!(
        workflows_list_tool.description.contains("chooseByOutcome"),
        "workflows_list should advertise outcome-based workflow selection: {}",
        workflows_list_tool.description
    );

    let build_start_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_BUILD_START)
        .expect("app_build_start tool exists");
    assert!(
        build_start_tool.description.contains("server-side draft")
            && build_start_tool.description.contains("one file at a time")
            && build_start_tool.input_schema.contains(r#""withUi""#),
        "app_build_start should advertise staged weak-model drafting: {} / {}",
        build_start_tool.description,
        build_start_tool.input_schema
    );

    let build_commit_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_BUILD_COMMIT)
        .expect("app_build_commit tool exists");
    assert!(
        build_commit_tool.description.contains("validationToken")
            && build_commit_tool.description.contains("app.add")
            && build_commit_tool.input_schema.contains(r#""draftId""#),
        "app_build_commit should advertise tokened commit through app.add: {} / {}",
        build_commit_tool.description,
        build_commit_tool.input_schema
    );
    assert!(
        !build_commit_tool.input_schema.contains("replaceExisting"),
        "app_build_commit schema must not advertise the refused replaceExisting flag: {}",
        build_commit_tool.input_schema
    );

    let build_start_schema = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_BUILD_START)
        .expect("app_build_start tool exists")
        .input_schema;
    assert!(
        build_start_schema.contains(r#""enum":["js_kv_app","js_kv_notes","js_multicap_audit"]"#),
        "app_build_start kind should be a closed enum: {build_start_schema}"
    );

    let build_list_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_BUILD_LIST)
        .expect("app_build_list tool exists");
    assert!(
        build_list_tool
            .description
            .contains("recover a lost draftId"),
        "app_build_list should advertise stall recovery: {}",
        build_list_tool.description
    );

    let put_file_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_BUILD_PUT_FILE)
        .expect("app_build_put_file tool exists");
    assert!(
        put_file_tool.input_schema.contains(r#""files""#)
            && put_file_tool
                .description
                .contains("several files in one call")
            && put_file_tool
                .input_schema
                .contains(r#""required":["draftId"]"#),
        "app_build_put_file should advertise single and batch modes: {} / {}",
        put_file_tool.description,
        put_file_tool.input_schema
    );

    // The initialize instructions are the one string most clients inject into
    // the model's system prompt — lock the load-bearing contracts into it.
    for token in [
        "app_build_start",
        "app_build_put_file",
        "app_build_validate",
        "app_build_commit",
        "function handle(input)",
        "window.terrane.invoke",
        "permission_required",
        "app_build_list",
        "never an object",
    ] {
        assert!(
            MCP_SERVER_INSTRUCTIONS.contains(token),
            "MCP_SERVER_INSTRUCTIONS should mention {token}"
        );
    }

    let inline_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_REGISTER_INLINE)
        .expect("app_register_inline tool exists");
    assert!(
        inline_tool.description.contains("Legacy")
            && inline_tool.description.contains("app.remove")
            && inline_tool.description.contains("JSON array")
            && inline_tool.description.contains("draftId")
            && inline_tool.input_schema.contains(r#""files""#),
        "app_register_inline should advertise MCP-only registration: {} / {}",
        inline_tool.description,
        inline_tool.input_schema
    );
    assert!(
        inline_tool.input_schema.contains("Do not JSON-stringify")
            && inline_tool.input_schema.contains("complete bundle"),
        "app_register_inline schema should warn against stringified or partial files: {}",
        inline_tool.input_schema
    );

    let recipe_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_RECIPE)
        .expect("app_recipe tool exists");
    assert!(
        recipe_tool.description.contains("withUi:true")
            && recipe_tool.description.contains("window.terrane.invoke")
            && recipe_tool.description.contains("kvGetOrNull"),
        "app_recipe should advertise UI app guidance: {}",
        recipe_tool.description
    );

    let scaffold_tool = tools
        .iter()
        .find(|tool| tool.name == TOOL_APP_SCAFFOLD)
        .expect("app_scaffold tool exists");
    assert!(
        scaffold_tool.description.contains("withUi:true")
            && scaffold_tool.description.contains("natural-language")
            && scaffold_tool.description.contains("defensive KV reads"),
        "app_scaffold should advertise visible app scaffolding: {}",
        scaffold_tool.description
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
            ("GET", "/apps/{id}/logs"),
        ]
    );

    let tool_names: Vec<&str> = c.mcp_tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        tool_names,
        vec![
            TOOL_WORKFLOWS_LIST,
            TOOL_WORKFLOW_INFO,
            TOOL_APP_RECIPE,
            TOOL_APP_BUILD_START,
            TOOL_APP_BUILD_PUT_FILE,
            TOOL_APP_BUILD_GET,
            TOOL_APP_BUILD_LIST,
            TOOL_APP_BUILD_VALIDATE,
            TOOL_APP_BUILD_COMMIT,
            TOOL_APP_BUILD_DISCARD,
            TOOL_APP_SCAFFOLD,
            TOOL_APP_BUNDLE_VALIDATE,
            TOOL_APP_REGISTER_INLINE,
            TOOL_APP_REGISTER,
            TOOL_APP_UPGRADE,
            TOOL_LIST_APPS,
            TOOL_APP_ACTIONS,
            TOOL_INVOKE,
            TOOL_APP_LOGS,
            TOOL_PERMISSION_CHECK,
            TOOL_PERMISSION_CANCEL,
            TOOL_PERMISSION_REQUESTS,
            TOOL_CAPABILITIES_LIST,
            TOOL_CAPABILITY_INFO,
            TOOL_CAPABILITY_QUERY,
            TOOL_CAPABILITY_COMMAND
        ]
    );

    let resources: Vec<&str> = c.mcp_resources.iter().map(|r| r.uri.as_str()).collect();
    assert!(resources.contains(&"terrane://docs/index"));
    assert!(resources.contains(&"terrane://docs/app-building"));

    let templates: Vec<&str> = c
        .mcp_resource_templates
        .iter()
        .map(|r| r.uri_template.as_str())
        .collect();
    assert!(templates.contains(&"terrane://capabilities/{namespace}"));

    let prompts: Vec<&str> = c.mcp_prompts.iter().map(|p| p.name.as_str()).collect();
    assert!(prompts.contains(&"make_js_kv_app"));
    assert!(
        !prompts.contains(&"make_js_multicap_app"),
        "weak-model eval prompts should stay outside the served MCP prompt surface"
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
            grant_resources: vec![],
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
