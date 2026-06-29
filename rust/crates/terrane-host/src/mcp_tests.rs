use super::{handle_json_rpc, json_string_value, top_level_fields};

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
        actions.contains(r#""structuredContent""#) && actions.contains("write"),
        "app_actions: {actions}"
    );
}
