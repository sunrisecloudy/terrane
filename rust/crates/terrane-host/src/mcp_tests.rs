use super::{handle_json_rpc, json_string_value, top_level_fields};

fn response_json_field(raw: &str, field: &str) -> String {
    let key = format!(r#""{field}":""#);
    let start = raw
        .find(&key)
        .unwrap_or_else(|| panic!("missing field {field} in {raw}"))
        + key.len();
    let rest = &raw[start..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("unterminated field {field} in {raw}"));
    rest[..end].to_string()
}

#[test]
fn top_level_parser_ignores_nested_ids() {
    let raw = r#"{"jsonrpc":"2.0","method":"ping","params":{"item":{"id":555}},"id":8}"#;
    let fields = top_level_fields(raw);
    let field = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

    assert_eq!(field("id"), Some("8"));
    assert_eq!(field("method").and_then(json_string_value), Some("ping"));
    assert_eq!(field("params"), Some(r#"{"item":{"id":555}}"#));
}

#[test]
fn capability_doc_tools_return_public_and_internal_views() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let list = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"capabilities_list","arguments":{}}}"#,
    )
    .unwrap();
    assert!(list.contains("relational_db"), "capabilities_list: {list}");
    assert!(list.contains("document"), "capabilities_list: {list}");

    let public = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"relational_db","#,
            r#""format":"json"}}}"#
        ),
    )
    .unwrap();
    assert!(
        public.contains("terrane.relational_db.tableSpec.v1"),
        "public: {public}"
    );
    assert!(!public.contains("Reserved kv layout"), "public: {public}");

    let internal = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"relational_db","#,
            r#""format":"json","includeInternal":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        internal.contains("Reserved kv layout"),
        "internal: {internal}"
    );

    let document = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"document","#,
            r#""format":"json"}}}"#
        ),
    )
    .unwrap();
    assert!(
        document.contains("document.schema.json"),
        "document: {document}"
    );
}

#[test]
fn capability_command_and_query_tools_use_core_without_protocol_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let help = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"app.add","#,
            r#""help":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        help.contains(r#""isError":false"#)
            && help.contains(r#"\"argument_order\":[\"id\",\"name\",\"source\",\"runtime\"]"#)
            && help.contains("--source")
            && help.contains("help:true never dispatches"),
        "command help: {help}"
    );

    let dry_run = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"app.add","#,
            r#""args":["demo","Demo App"],"dryRun":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        dry_run.contains(r#"\"dryRun\":true"#) && dry_run.contains(r#""isError":false"#),
        "dry run: {dry_run}"
    );

    let before = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"#,
            r#""name":"capability_query","arguments":{"capability":"app","#,
            r#""query":"app.exists","args":["demo"]}}}"#
        ),
    )
    .unwrap();
    assert!(
        before.contains(r#"\"value\":false"#) && before.contains(r#""isError":false"#),
        "query before commit: {before}"
    );

    let committed = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"app.add","#,
            r#""args":["demo","Demo App"]}}}"#
        ),
    )
    .unwrap();
    assert!(
        committed.contains(r#"\"records\":1"#) && committed.contains(r#""isError":false"#),
        "commit: {committed}"
    );

    let after = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"#,
            r#""name":"capability_query","arguments":{"capability":"app","#,
            r#""query":"exists","args":["demo"]}}}"#
        ),
    )
    .unwrap();
    assert!(
        after.contains(r#"\"value\":true"#) && after.contains(r#""isError":false"#),
        "query after commit: {after}"
    );

    let invalid_args = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"app.add","#,
            r#""args":"not-an-array"}}}"#
        ),
    )
    .unwrap();
    assert!(
        invalid_args.contains(r#""isError":true"#)
            && invalid_args.contains("array of strings")
            && !invalid_args.contains(r#""error":"#),
        "invalid args should be a tool error, not protocol error: {invalid_args}"
    );

    let unsupported_dry_run = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"net.fetch","#,
            r#""args":["demo","https://example.test"],"dryRun":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        unsupported_dry_run.contains(r#""isError":true"#)
            && unsupported_dry_run.contains("dryRun unsupported"),
        "unsupported dryRun: {unsupported_dry_run}"
    );
}

#[test]
fn weak_model_workflows_app_helpers_and_structured_results_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let tools = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":20,"method":"tools/list"}"#,
    )
    .unwrap();
    assert!(
        tools.contains("workflows_list")
            && tools.contains("workflow_info")
            && tools.contains("app_scaffold")
            && tools.contains("app_bundle_validate")
            && tools.contains("app_register"),
        "tools/list: {tools}"
    );
    assert!(
        tools.contains("permission_check")
            && tools.contains("permission_cancel")
            && tools.contains("permission_requests"),
        "tools/list permission tools: {tools}"
    );

    let direct_tool_mistake = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":21,"method":"list_apps","params":{}}"#,
    )
    .unwrap();
    assert!(
        direct_tool_mistake.contains("tools/call") && direct_tool_mistake.contains("list_apps"),
        "direct tool correction: {direct_tool_mistake}"
    );

    let workflows = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"workflows_list","arguments":{}}}"#,
    )
    .unwrap();
    assert!(
        workflows.contains("make_js_kv_app")
            && workflows.contains(r#""structuredContent""#)
            && workflows.contains("Always call MCP tools"),
        "workflows_list: {workflows}"
    );

    let workflow = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"workflow_info","arguments":{"name":"make_js_kv_app"}}}"#,
    )
    .unwrap();
    assert!(
        workflow.contains("app_bundle_validate") && workflow.contains("app_register"),
        "workflow_info: {workflow}"
    );

    let scaffold = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"app_scaffold","arguments":{"id":"weak-demo","name":"Weak Demo"}}}"#,
    )
    .unwrap();
    assert!(
        scaffold.contains("manifest.json")
            && scaffold.contains("main.js")
            && scaffold.contains(r#""structuredContent""#),
        "app_scaffold: {scaffold}"
    );

    let app_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        app_dir.path().join("manifest.json"),
        r#"{"id":"weak-demo","name":"Weak Demo","runtime":"js","backend":"main.js","resources":["kv"]}"#,
    )
    .unwrap();
    std::fs::write(
        app_dir.path().join("main.js"),
        r#"function handle(input) {
  var verb = input[0] || "";
  var kv = ctx.resource.kv;
  if (verb === "__actions__") {
    return JSON.stringify({app:"weak-demo",actions:[
      {verb:"write",args:[{name:"text",required:true}],returns:"stored"},
      {verb:"read",args:[],returns:"text"}
    ]});
  }
  if (verb === "write") { kv.set("note", input.slice(1).join(" ")); return "stored"; }
  if (verb === "read") { var note = kv.get("note"); return note == null ? "(empty)" : note; }
  return "unknown";
}"#,
    )
    .unwrap();
    let path = app_dir.path().to_string_lossy();

    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{{"name":"app_bundle_validate","arguments":{{"path":{}}}}}}}"#,
            super::json_str(&path)
        ),
    )
    .unwrap();
    assert!(
        validate.contains(r#""valid":true"#) && validate.contains(r#""structuredContent""#),
        "app_bundle_validate: {validate}"
    );

    let dry_run = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{{"name":"app_register","arguments":{{"source":{},"dryRun":true}}}}}}"#,
            super::json_str(&path)
        ),
    )
    .unwrap();
    assert!(
        dry_run.contains(r#"\"dryRun\":true"#) && dry_run.contains(r#""isError":false"#),
        "app_register dryRun: {dry_run}"
    );

    let commit = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":27,"method":"tools/call","params":{{"name":"app_register","arguments":{{"source":{}}}}}}}"#,
            super::json_str(&path)
        ),
    )
    .unwrap();
    assert!(
        commit.contains(r#""command":"app.add"#) && commit.contains(r#"\"records\":1"#),
        "app_register commit: {commit}"
    );

    let denied_actions = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-actions-denied","method":"tools/call","params":{"name":"app_actions","arguments":{"app":"weak-demo"}}}"#,
    )
    .unwrap();
    assert!(
        denied_actions.contains("permission_required")
            && denied_actions.contains("adminUrl")
            && denied_actions.contains(r#"\"source\":\"mcp_stdio\""#)
            && denied_actions.contains(r#""requestStatus":"pending"#)
            && denied_actions.contains("permission_check"),
        "app_actions should return structured permission request: {denied_actions}"
    );
    let request_id = response_json_field(&denied_actions, "requestId");

    let request_check = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"weak-permission-check","method":"tools/call","params":{{"name":"permission_check","arguments":{{"requestId":{}}}}}}}"#,
            super::json_str(&request_id)
        ),
    )
    .unwrap();
    assert!(
        request_check.contains(r#""status":"pending"#) && request_check.contains("weak-demo"),
        "permission_check: {request_check}"
    );

    let requests = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-permission-list","method":"tools/call","params":{"name":"permission_requests","arguments":{}}}"#,
    )
    .unwrap();
    assert!(
        requests.contains(&request_id) && requests.contains(r#""status":"pending"#),
        "permission_requests: {requests}"
    );

    let cancel = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"weak-permission-cancel","method":"tools/call","params":{{"name":"permission_cancel","arguments":{{"requestId":{},"reason":"test"}}}}}}"#,
            super::json_str(&request_id)
        ),
    )
    .unwrap();
    assert!(
        cancel.contains(r#""status":"cancelled"#),
        "permission_cancel: {cancel}"
    );

    let grant = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-grant","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"auth.grant","args":["user:local-owner","weak-demo","kv"]}}}"#,
    )
    .unwrap();
    assert!(
        grant.contains(r#""isError":true"#) && grant.contains("trusted-admin-only"),
        "auth.grant weak-demo should be blocked: {grant}"
    );

    let apps = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":28,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#,
    )
    .unwrap();
    assert!(
        apps.contains("weak-demo") && apps.contains(r#""structuredContent""#),
        "list_apps: {apps}"
    );

    let actions = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":29,"method":"tools/call","params":{"name":"app_actions","arguments":{"app":"weak-demo"}}}"#,
    )
    .unwrap();
    assert!(
        actions.contains("permission_required") && actions.contains("permission_check"),
        "app_actions should keep routing through permission request flow: {actions}"
    );
}

// --- In-session approval: elicitation helpers -----------------------------

#[test]
fn initialize_elicitation_capability_is_detected() {
    let with = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"elicitation":{}}}}"#;
    let without = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    let other = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    assert!(super::initialize_declares_elicitation(with));
    assert!(!super::initialize_declares_elicitation(without));
    assert!(!super::initialize_declares_elicitation(other));
}

#[test]
fn permission_required_response_yields_elicit_info() {
    let response = r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"x"}],"structuredContent":{"type":"permission_required","requestId":"local-demo-kv","app":"demo","appName":"Demo","missingResources":["kv","crdt"],"adminUrl":"http://127.0.0.1:8780/__terrane/admin/requests/local-demo-kv"},"isError":true}}"#;
    let info = super::permission_required_from_tool_response(response).expect("elicit info");
    assert_eq!(info.request_id, "local-demo-kv");
    assert_eq!(info.app, "demo");
    assert_eq!(info.app_name, "Demo");
    assert_eq!(info.missing_resources, vec!["kv".to_string(), "crdt".to_string()]);
    assert!(info.admin_url.ends_with("/local-demo-kv"));

    // An ordinary (non-permission) result yields nothing to elicit.
    let ok = r#"{"jsonrpc":"2.0","id":6,"result":{"content":[{"type":"text","text":"done"}],"isError":false}}"#;
    assert!(super::permission_required_from_tool_response(ok).is_none());
}

#[test]
fn elicitation_frame_is_a_wellformed_create_request() {
    let info = super::ElicitInfo {
        request_id: "local-demo-kv".into(),
        app: "demo".into(),
        app_name: "Demo".into(),
        missing_resources: vec!["kv".into(), "crdt".into()],
        admin_url: "http://127.0.0.1:8780/__terrane/admin/requests/local-demo-kv".into(),
    };
    let frame = super::elicitation_create_frame("terrane-elicit-1", &info);
    assert!(frame.contains(r#""method":"elicitation/create""#), "{frame}");
    assert!(frame.contains(r#""id":"terrane-elicit-1""#), "{frame}");
    assert!(frame.contains("Demo") && frame.contains("kv, crdt"), "{frame}");
    assert!(frame.contains(r#""enum":["approve","deny"]"#), "{frame}");
    // Valid JSON.
    let _: serde_json::Value = serde_json::from_str(&frame).expect("frame is JSON");
}

#[test]
fn elicitation_decision_covers_accept_decline_and_mismatch() {
    let id = "terrane-elicit-1";
    let accept_approve = r#"{"jsonrpc":"2.0","id":"terrane-elicit-1","result":{"action":"accept","content":{"decision":"approve"}}}"#;
    let accept_deny = r#"{"jsonrpc":"2.0","id":"terrane-elicit-1","result":{"action":"accept","content":{"decision":"deny"}}}"#;
    let decline = r#"{"jsonrpc":"2.0","id":"terrane-elicit-1","result":{"action":"decline"}}"#;
    let cancel = r#"{"jsonrpc":"2.0","id":"terrane-elicit-1","result":{"action":"cancel"}}"#;
    let err = r#"{"jsonrpc":"2.0","id":"terrane-elicit-1","error":{"code":-1,"message":"no"}}"#;
    let other_id = r#"{"jsonrpc":"2.0","id":"terrane-elicit-2","result":{"action":"accept","content":{"decision":"approve"}}}"#;

    assert_eq!(super::elicitation_decision(accept_approve, id), Some(super::ElicitDecision::Approve));
    assert_eq!(super::elicitation_decision(accept_deny, id), Some(super::ElicitDecision::Deny));
    assert_eq!(super::elicitation_decision(decline, id), Some(super::ElicitDecision::Deny));
    assert_eq!(super::elicitation_decision(cancel, id), Some(super::ElicitDecision::Deny));
    assert_eq!(super::elicitation_decision(err, id), Some(super::ElicitDecision::Deny));
    // A different id is not our response — keep waiting.
    assert_eq!(super::elicitation_decision(other_id, id), None);
}

#[test]
fn busy_error_replies_to_requests_but_ignores_notifications() {
    let request = r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#;
    let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let reply = super::busy_error(request).expect("busy error for a request");
    assert!(reply.contains(r#""id":9"#) && reply.contains("awaiting an elicitation"), "{reply}");
    assert!(super::busy_error(notification).is_none());
    assert_eq!(super::parsed_method(request).as_deref(), Some("tools/list"));
}
