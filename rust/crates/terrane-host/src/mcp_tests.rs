use super::{
    handle_json_rpc, handle_json_rpc_with_source_and_admin_base, json_string_value,
    top_level_fields,
};
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn structured_content(raw: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(raw).unwrap()["result"]["structuredContent"].clone()
}

struct EnvRestore {
    old_home: Option<OsString>,
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.old_home {
            Some(value) => std::env::set_var("TERRANE_HOME", value),
            None => std::env::remove_var("TERRANE_HOME"),
        }
    }
}

fn isolate_home(home: &std::path::Path) -> EnvRestore {
    let old_home = std::env::var_os("TERRANE_HOME");
    std::env::set_var("TERRANE_HOME", home);
    EnvRestore { old_home }
}

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
fn app_logs_tool_reads_local_app_buffer() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();
    crate::app_log::append(dir.path(), "demo", "warn", "careful", "{}").unwrap();

    let raw = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"app_logs","arguments":{"app":"demo","level":"warn","tail":5}}}"#,
    )
    .unwrap();
    let content = structured_content(&raw);

    assert_eq!(content["lines"][0]["level"], "warn");
    assert_eq!(content["lines"][0]["msg"], "careful");
    assert!(raw.contains(r#""isError":false"#), "app_logs: {raw}");
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
            && unsupported_dry_run.contains("not available through untrusted capability_command"),
        "unsupported dryRun: {unsupported_dry_run}"
    );
}

#[test]
fn capability_command_resource_writes_require_permission_and_can_retry_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();
    crate::dispatch_on_core(&mut core, "app.add", &["demo".into(), "Demo".into()]).unwrap();

    let denied = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":"kv-denied","method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"kv.set","#,
            r#""args":["demo","key","value"]}}}"#
        ),
    )
    .unwrap();
    assert!(
        denied.contains("permission_required")
            && denied.contains(r#""operation":"capability_command:kv.set""#)
            && denied.contains(r#""source":"mcp_stdio""#)
            && denied.contains(r#""requestStatus":"pending""#)
            && denied.contains(r#""operatorActionRequired":true"#)
            && denied.contains(r#""allowedMcpTools":["permission_check","permission_requests","permission_cancel"]"#)
            && denied.contains(r#""forbiddenMcpTools":["capability_command:auth.*""#)
            && denied.contains(r#""nextModelAction":"Do not call capability_command"#)
            && denied.contains(r#""missingResources":["kv"]"#),
        "kv.set denial should be structured permission_required: {denied}"
    );
    let request_id = response_json_field(&denied, "requestId");
    assert!(
        super::permission_required_from_tool_response(&denied).is_some(),
        "pending command request should be eligible for elicitation"
    );

    crate::permission::approve_permission_request(
        &mut core,
        &request_id,
        "ok",
        crate::permission::DEFAULT_ADMIN_BASE_URL,
    )
    .unwrap()
    .expect("approve request");

    let retry = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":"kv-retry","method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"kv.set","#,
            r#""args":["demo","key","value"]}}}"#
        ),
    )
    .unwrap();
    assert!(
        retry.contains(r#""isError":false"#) && retry.contains(r#"\"records\":1"#),
        "kv.set retry should succeed after approval: {retry}"
    );
}

#[test]
fn capability_command_dry_run_permission_preview_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();
    crate::dispatch_on_core(&mut core, "app.add", &["demo".into(), "Demo".into()]).unwrap();
    let before = terrane_cap_auth::permission_requests(core.state())
        .unwrap()
        .len();

    let preview = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":"kv-preview","method":"tools/call","params":{"#,
            r#""name":"capability_command","arguments":{"name":"kv.rm","#,
            r#""args":["demo","missing-key"],"dryRun":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        preview.contains("permission_required")
            && preview.contains(r#""operation":"capability_command:kv.rm""#)
            && preview.contains(r#""requestStatus":"preview""#)
            && preview.contains("rerun without dryRun")
            && !preview.contains("KeyNotFound"),
        "dryRun should return preview permission without decide leak: {preview}"
    );
    assert!(
        super::permission_required_from_tool_response(&preview).is_none(),
        "preview-only permission responses must not trigger elicitation"
    );
    let after = terrane_cap_auth::permission_requests(core.state())
        .unwrap()
        .len();
    assert_eq!(after, before, "dryRun preview must not record a request");
}

#[test]
fn capability_command_refuses_effect_runtime_storage_and_destructive_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    for (command, args, expected) in [
        (
            "kv.storage.set",
            vec!["default", "sqlite"],
            "storage configuration is trusted-admin-only",
        ),
        (
            "js-runtime.run",
            vec!["demo"],
            "run apps through the invoke tool",
        ),
        (
            "net.fetch",
            vec!["https://example.test"],
            "not available through untrusted capability_command",
        ),
        (
            "model.ask",
            vec!["hello"],
            "not available through untrusted capability_command",
        ),
        (
            "harness.generate-app",
            vec!["calendar"],
            "trusted tooling and cannot run",
        ),
        (
            "app.import",
            vec![
                "/tmp/missing-bundle",
                "--storage",
                "sqlite",
                "--path",
                "/tmp/evil.db",
            ],
            "app.import installs bundles",
        ),
        (
            "app.remove",
            vec!["demo"],
            "destructive and trusted-admin-only",
        ),
    ] {
        let args_json = args
            .iter()
            .map(|arg| super::json_str(arg))
            .collect::<Vec<_>>()
            .join(",");
        let response = handle_json_rpc(
            &mut core,
            &format!(
                r#"{{"jsonrpc":"2.0","id":"refuse","method":"tools/call","params":{{"name":"capability_command","arguments":{{"name":{},"args":[{}]}}}}}}"#,
                super::json_str(command),
                args_json
            ),
        )
        .unwrap();
        assert!(
            response.contains(r#""isError":true"#) && response.contains(expected),
            "{command} should be refused: {response}"
        );
    }
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
    assert!(
        scaffold.contains(r#""nextToolCallSource""#)
            && scaffold.contains(r#""from":"this_result.structuredContent.files""#)
            && scaffold.contains(r#""doNotJsonStringify":true"#),
        "app_scaffold should tell weak models how to pass files array: {scaffold}"
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
      {verb:"read",args:[],returns:"text"},
      {verb:"common.receive",args:[{name:"kind",required:false},{name:"payloadJson",required:false}],returns:"JSON status"},
      {verb:"common.list",args:[],returns:"JSON array"},
      {verb:"common.get",args:[{name:"id",required:true}],returns:"JSON item or typed not found"}
    ]});
  }
  if (verb === "write") { kv.set("note", input.slice(1).join(" ")); return "stored"; }
  if (verb === "read") { var note = kv.get("note"); return note == null ? "(empty)" : note; }
  if (verb === "common.receive") { kv.set("inbox/latest", input[2] || ""); return JSON.stringify({ok:true}); }
  if (verb === "common.list") { return JSON.stringify([]); }
  if (verb === "common.get") { return JSON.stringify({error:{code:"NotFound",message:"item not found"}}); }
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

    let invalid_invoke_args = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-invoke-bad-args","method":"tools/call","params":{"name":"invoke","arguments":{"app":"weak-demo","verb":"write","args":"[\"hello\"]"}}}"#,
    )
    .unwrap();
    assert!(
        invalid_invoke_args.contains(r#""isError":true"#)
            && invalid_invoke_args.contains("real JSON array of strings")
            && invalid_invoke_args.contains(r#"\"app\":\"weak-demo\""#)
            && invalid_invoke_args.contains(r#"\"verb\":\"write\""#)
            && invalid_invoke_args.contains(r#""args\":[\"{...}\"]"#),
        "invoke args error should repair stringified arrays: {invalid_invoke_args}"
    );

    let custom_admin_base = "http://127.0.0.1:49199";
    let denied_actions = handle_json_rpc_with_source_and_admin_base(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-actions-denied","method":"tools/call","params":{"name":"app_actions","arguments":{"app":"weak-demo"}}}"#,
        "mcp_stdio",
        custom_admin_base,
    )
    .unwrap();
    assert!(
        denied_actions.contains("permission_required")
            && denied_actions.contains("adminUrl")
            && denied_actions.contains(custom_admin_base)
            && denied_actions.contains(r#"\"source\":\"mcp_stdio\""#)
            && denied_actions.contains(r#""requestStatus":"pending"#)
            && denied_actions.contains(r#""operatorActionRequired":true"#)
            && denied_actions.contains(r#""forbiddenMcpTools":["capability_command:auth.*""#)
            && denied_actions.contains("permission_check"),
        "app_actions should return structured permission request: {denied_actions}"
    );
    let request_id = response_json_field(&denied_actions, "requestId");

    let request_check = handle_json_rpc_with_source_and_admin_base(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"weak-permission-check","method":"tools/call","params":{{"name":"permission_check","arguments":{{"requestId":{}}}}}}}"#,
            super::json_str(&request_id)
        ),
        "mcp_stdio",
        custom_admin_base,
    )
    .unwrap();
    assert!(
        request_check.contains(r#""status":"pending"#)
            && request_check.contains("weak-demo")
            && request_check.contains(custom_admin_base),
        "permission_check: {request_check}"
    );

    let requests = handle_json_rpc_with_source_and_admin_base(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"weak-permission-list","method":"tools/call","params":{"name":"permission_requests","arguments":{}}}"#,
        "mcp_stdio",
        custom_admin_base,
    )
    .unwrap();
    assert!(
        requests.contains(&request_id)
            && requests.contains(r#""status":"pending"#)
            && requests.contains(custom_admin_base),
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

#[test]
fn app_build_staged_tools_validate_and_commit_without_resending_files() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let log = dir.path().join("log.bin");
    let mut core = crate::open_at_log_path(&log).unwrap();

    let tools = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"tools","method":"tools/list"}"#,
    )
    .unwrap();
    assert!(
        tools.contains("app_build_start")
            && tools.contains("app_build_put_file")
            && tools.contains("app_build_validate")
            && tools.contains("app_build_commit"),
        "tools/list should include staged build tools: {tools}"
    );

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"build-start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"staged-demo","name":"Staged Demo","withUi":true}}}"#,
    )
    .unwrap();
    let start_content = structured_content(&start);
    let draft_id = start_content["draftId"].as_str().unwrap().to_string();
    assert!(
        start_content["files"].as_array().unwrap().len() >= 4,
        "start content: {start_content}"
    );

    let unsafe_path = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"unsafe","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"../main.js","content":"bad"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert!(
        unsafe_path.contains(r#""isError":true"#) && unsafe_path.contains("safe relative"),
        "unsafe path should be a tool error: {unsafe_path}"
    );

    let main_js = r#"function handle(input) {
  var verb = input[0] || "";
  var kv = ctx.resource.kv;
  if (verb === "__actions__") {
    return JSON.stringify({app:"staged-demo",actions:[
      {verb:"write",args:[{name:"text",required:true}],returns:"stored"},
      {verb:"read",args:[],returns:"text"},
      {verb:"common.receive",args:[{name:"kind",required:false},{name:"payloadJson",required:false}],returns:"JSON status"},
      {verb:"common.list",args:[],returns:"JSON array"},
      {verb:"common.get",args:[{name:"id",required:true}],returns:"JSON item or typed not found"}
    ]});
  }
  if (verb === "write") { kv.set("note", input.slice(1).join(" ")); return "stored"; }
  if (verb === "read") { try { return kv.get("note"); } catch (err) { return "(empty)"; } }
  if (verb === "common.receive") { kv.set("inbox/latest", input[2] || ""); return JSON.stringify({ok:true}); }
  if (verb === "common.list") { return JSON.stringify([]); }
  if (verb === "common.get") { return JSON.stringify({error:{code:"NotFound",message:"item not found"}}); }
  return "unknown";
}"#;
    let put = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put-main","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"main.js","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(main_js)
        ),
    )
    .unwrap();
    assert!(
        put.contains(r#""isError":false"#) && put.contains("app_build_validate"),
        "put main.js: {put}"
    );

    let records_before_validate = terrane_core::read_log(&log).unwrap().len();
    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"validate","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let records_after_validate = terrane_core::read_log(&log).unwrap().len();
    assert_eq!(
        records_after_validate, records_before_validate,
        "app_build_validate must not append records"
    );
    let validation = structured_content(&validate);
    assert_eq!(validation["valid"], true, "validate: {validate}");
    let token = validation["validationToken"].as_str().unwrap().to_string();

    let commit = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"commit","method":"tools/call","params":{{"name":"app_build_commit","arguments":{{"draftId":{},"validationToken":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(&token)
        ),
    )
    .unwrap();
    let committed = structured_content(&commit);
    assert_eq!(committed["records"], 1, "commit: {commit}");
    assert_eq!(committed["draftDiscarded"], true, "commit: {commit}");
    assert!(
        dir.path()
            .join("apps")
            .join("staged-demo")
            .join("main.js")
            .is_file(),
        "owned app bundle should be written"
    );
    assert!(
        !dir.path().join(".mcp-drafts").join(&draft_id).exists(),
        "committed draft should be removed"
    );

    let apps = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"apps","method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#,
    )
    .unwrap();
    assert!(apps.contains("staged-demo"), "list_apps: {apps}");
}

#[test]
fn app_register_inline_dry_run_can_commit_by_draft_id() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();
    let files = serde_json::json!([
        {
            "path": "manifest.json",
            "content": "{\"id\":\"inline-draft\",\"name\":\"Inline Draft\",\"runtime\":\"js\",\"backend\":\"main.js\",\"resources\":[\"kv\"]}"
        },
        {
            "path": "main.js",
            "content": "function handle(input){var verb=input[0]||'';if(verb==='__actions__'){return JSON.stringify({app:'inline-draft',actions:[{verb:'read',args:[],returns:'ok'},{verb:'common.receive',args:[],returns:'JSON status'},{verb:'common.list',args:[],returns:'JSON array'},{verb:'common.get',args:[{name:'id',required:true}],returns:'JSON item or typed not found'}]});}if(verb==='common.receive'){return JSON.stringify({ok:true});}if(verb==='common.list'){return JSON.stringify([]);}if(verb==='common.get'){return JSON.stringify({error:{code:'NotFound',message:'item not found'}});}return 'ok';}"
        }
    ]);
    let dry_run = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"inline-dry","method":"tools/call","params":{{"name":"app_register_inline","arguments":{{"dryRun":true,"files":{}}}}}}}"#,
            files
        ),
    )
    .unwrap();
    let dry = structured_content(&dry_run);
    assert_eq!(dry["dryRun"], true, "inline dryRun: {dry_run}");
    let draft_id = dry["draftId"].as_str().unwrap();
    let token = dry["validationToken"].as_str().unwrap();
    assert!(
        dry_run.contains("app_build_commit") && dry_run.contains("do not resend the files"),
        "inline dryRun should point to staged commit: {dry_run}"
    );

    let commit = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"inline-commit","method":"tools/call","params":{{"name":"app_build_commit","arguments":{{"draftId":{},"validationToken":{}}}}}}}"#,
            super::json_str(draft_id),
            super::json_str(token)
        ),
    )
    .unwrap();
    assert!(
        commit.contains(r#""isError":false"#)
            && commit.contains(r#""command":"app.add"#)
            && commit.contains("inline-draft"),
        "commit from inline draft: {commit}"
    );
    assert!(
        dir.path()
            .join("apps")
            .join("inline-draft")
            .join("main.js")
            .is_file(),
        "inline draft should install app files"
    );
}

#[test]
fn initialize_result_carries_weak_model_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();
    let init = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    )
    .unwrap();
    assert!(
        init.contains(r#""instructions":"#),
        "initialize should carry server instructions: {init}"
    );
    for token in [
        "app_build_start",
        "handle(input)",
        "window.terrane.invoke",
        "permission_required",
        "app_build_list",
    ] {
        assert!(
            init.contains(token),
            "instructions should mention {token}: {init}"
        );
    }
}

#[test]
fn app_build_validate_rejects_runtime_incompatible_backends() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"contract-demo","name":"Contract Demo"}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    let validate = |core: &mut crate::HostCore| {
        structured_content(
            &handle_json_rpc(
                core,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
                    super::json_str(&draft_id)
                ),
            )
            .unwrap(),
        )
    };
    let put_main = |core: &mut crate::HostCore, content: &str| {
        handle_json_rpc(
            core,
            &format!(
                r#"{{"jsonrpc":"2.0","id":"p","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"main.js","content":{}}}}}}}"#,
                super::json_str(&draft_id),
                super::json_str(content)
            ),
        )
        .unwrap()
    };

    // Deno/Node-style module backend: plausible, but the runtime cannot run it.
    put_main(
        &mut core,
        "import { serve } from 'https://deno.land/std/http/server.ts';\nexport async function addEvent(e) { return 'ok'; }",
    );
    let module = validate(&mut core);
    assert_eq!(module["valid"], false, "module backend: {module}");
    let module_errors = module["errors"].to_string();
    assert!(
        module_errors.contains("plain script") && module_errors.contains("import/export"),
        "module backend errors should explain the plain-script contract: {module_errors}"
    );

    // No handle(input) and no actions table.
    put_main(&mut core, "function addEvent(e) { return 'ok'; }");
    let missing = validate(&mut core);
    assert_eq!(missing["valid"], false, "missing handle: {missing}");
    assert!(
        missing["errors"]
            .to_string()
            .contains("function handle(input)"),
        "missing-handle error should show the fix: {missing}"
    );

    // Lexical handle never lands on the global object.
    put_main(&mut core, "const handle = (input) => 'ok';");
    let lexical = validate(&mut core);
    assert_eq!(lexical["valid"], false, "const handle: {lexical}");
    assert!(
        lexical["errors"].to_string().contains("global object"),
        "const-handle error should explain the global requirement: {lexical}"
    );

    // Object-style dispatch is a warning with the positional contract spelled out.
    put_main(
        &mut core,
        "function handle(input) { var action = input.action || 'list'; var verb = input[0] || ''; if (verb === '__actions__') { return JSON.stringify({actions:[{verb:'common.receive',args:[],returns:'JSON status'},{verb:'common.list',args:[],returns:'JSON array'},{verb:'common.get',args:[{name:'id',required:true}],returns:'JSON item or typed not found'}]}); } if (verb === 'common.receive') { return JSON.stringify({ok:true}); } if (verb === 'common.list') { return JSON.stringify([]); } if (verb === 'common.get') { return JSON.stringify({error:{code:'NotFound',message:'item not found'}}); } return String(action); }",
    );
    let object_style = validate(&mut core);
    assert_eq!(object_style["valid"], true, "object style: {object_style}");
    assert!(
        object_style["warnings"]
            .to_string()
            .contains("array of strings"),
        "object-style warning should state the positional contract: {object_style}"
    );
}

#[test]
fn app_build_validate_explains_manifest_shape_errors() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"manifest-demo","name":"Manifest Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    // The observed weak-model mistake: a rich object-shaped ui field.
    let manifest = r#"{"id":"manifest-demo","name":"Manifest Demo","runtime":"js","backend":"main.js","ui":{"index":"index.html","scripts":["ui.js"],"styles":["style.css"]},"resources":["kv"]}"#;
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"manifest.json","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(manifest)
        ),
    )
    .unwrap();

    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validation = structured_content(&validate);
    assert_eq!(validation["valid"], false, "ui object: {validate}");
    let errors = validation["errors"].to_string();
    assert!(
        errors.contains("manifest.ui must be a string file path")
            && errors.contains("not an object"),
        "ui-object error should be prescriptive: {errors}"
    );
    assert!(
        errors.contains(r#"\"ui\":\"index.html\""#) || errors.contains(r#""ui":"index.html""#),
        "errors should include the corrected manifest shape: {errors}"
    );
}

#[test]
fn app_build_list_recovers_draft_ids() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let empty = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"empty","method":"tools/call","params":{"name":"app_build_list","arguments":{}}}"#,
    )
    .unwrap();
    let empty_content = structured_content(&empty);
    assert_eq!(
        empty_content["drafts"].as_array().map(Vec::len),
        Some(0),
        "empty list: {empty}"
    );
    assert!(
        empty.contains("app_build_start"),
        "empty list should route to app_build_start: {empty}"
    );

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"lost-draft","name":"Lost Draft"}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    let list = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"list","method":"tools/call","params":{"name":"app_build_list","arguments":{}}}"#,
    )
    .unwrap();
    let listed = structured_content(&list);
    let drafts = listed["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 1, "list after start: {list}");
    assert_eq!(drafts[0]["draftId"], draft_id.as_str(), "list: {list}");
    assert_eq!(drafts[0]["app"]["id"], "lost-draft", "list: {list}");
    assert!(
        list.contains("app_build_get"),
        "list should route back into the draft: {list}"
    );
    assert!(
        list.contains("do not read files first"),
        "list should route resumed models to validate-first: {list}"
    );
}

#[test]
fn ui_scaffold_passes_validate_untouched() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"shell-demo","name":"Shell Demo","withUi":true}}}"#,
    )
    .unwrap();
    let content = structured_content(&start);
    let draft_id = content["draftId"].as_str().unwrap().to_string();
    let paths: Vec<&str> = content["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            "index.html",
            "main.js",
            "manifest.json",
            "style.css",
            "ui.js"
        ],
        "shell scaffold files: {content}"
    );
    assert_eq!(content["kind"], "js_kv_app", "shell kind: {content}");
    assert!(
        content["contract"]["styleContract"]
            .as_str()
            .unwrap()
            .contains("light+dark"),
        "shell contract should advertise the design system: {content}"
    );

    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validation = structured_content(&validate);
    assert_eq!(validation["valid"], true, "untouched shell: {validate}");
    assert_eq!(
        validation["errors"],
        serde_json::Value::Null,
        "untouched shell should have no errors: {validate}"
    );
    // The only expected warning is the still-the-demo-app nudge.
    let warnings = validation["warnings"].as_array().unwrap();
    assert_eq!(
        warnings.len(),
        1,
        "untouched shell should warn exactly once: {validate}"
    );
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("unmodified scaffold demo app"),
        "untouched shell warning should name the pristine backend: {validate}"
    );
}

#[test]
fn ui_scaffold_shell_is_substantial() {
    let scaffold = super::app_scaffold_json("shell-demo", "Shell Demo", "js_kv_app", true).unwrap();
    let content: serde_json::Value = serde_json::from_str(&scaffold).unwrap();
    let file = |path: &str| -> String {
        content["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("missing {path}"))["content"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let css = file("style.css");
    assert!(css.len() > 2500, "style.css should be a real design system");
    for token in ["--accent", "prefers-color-scheme", ".empty-state", ".grid"] {
        assert!(css.contains(token), "style.css should contain {token}");
    }

    let ui = file("ui.js");
    for token in [
        "window.terrane.invoke(",
        "/* REPLACE:",
        "/* KEEP:",
        "setStatus",
    ] {
        assert!(ui.contains(token), "ui.js should contain {token}");
    }
    assert!(
        !ui.contains("\nimport ") && !ui.contains("\nexport "),
        "ui.js must not model module syntax"
    );

    let html = file("index.html");
    for token in [
        "<head>",
        "id=\"main-input\"",
        "id=\"status\"",
        "id=\"list\"",
        "id=\"empty\"",
        "Shell Demo",
    ] {
        assert!(html.contains(token), "index.html should contain {token}");
    }

    let main = file("main.js");
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    super::check_js_backend_contract("main.js", &main, &mut errors, &mut warnings);
    assert!(errors.is_empty(), "shell main.js lint errors: {errors:?}");
    assert!(
        warnings.is_empty(),
        "shell main.js lint warnings: {warnings:?}"
    );
    assert!(
        main.contains("kvGetOrNull"),
        "main.js keeps defensive reads"
    );
}

#[test]
fn ui_scaffold_escapes_app_name() {
    let scaffold =
        super::app_scaffold_json("esc-demo", "A <b>&\"quote\"</b>", "js_kv_app", true).unwrap();
    let content: serde_json::Value = serde_json::from_str(&scaffold).unwrap();
    let html = content["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "index.html")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(
        !html.contains("<b>") && html.contains("&lt;b&gt;"),
        "app name must be HTML-escaped: {html}"
    );
}

#[test]
fn app_build_put_file_batch_writes_all_or_nothing() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"batch-demo","name":"Batch Demo"}}}"#,
    )
    .unwrap();
    let start_content = structured_content(&start);
    let draft_id = start_content["draftId"].as_str().unwrap().to_string();
    let hash_before = start_content["bundleHash"].as_str().unwrap().to_string();

    // One unsafe entry rejects the whole batch and writes nothing.
    let bad = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"bad","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"files":[{{"path":"main.js","content":"function handle(input){{return 'x';}}"}},{{"path":"../evil.js","content":"x"}}]}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert!(
        bad.contains(r#""isError":true"#) && bad.contains("safe relative bundle path"),
        "unsafe batch entry: {bad}"
    );
    let get = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"get","method":"tools/call","params":{{"name":"app_build_get","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert_eq!(
        structured_content(&get)["bundleHash"].as_str().unwrap(),
        hash_before,
        "rejected batch must not change the draft: {get}"
    );

    // A good batch writes several files in one call.
    let good = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"good","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"files":[{{"path":"main.js","content":"function handle(input){{var verb=input[0]||'';if(verb==='__actions__'){{return JSON.stringify({{actions:[{{verb:'common.receive',args:[],returns:'JSON status'}},{{verb:'common.list',args:[],returns:'JSON array'}},{{verb:'common.get',args:[{{name:'id',required:true}}],returns:'JSON item or typed not found'}}]}});}}if(verb==='common.receive'){{return JSON.stringify({{ok:true}});}}if(verb==='common.list'){{return JSON.stringify([]);}}if(verb==='common.get'){{return JSON.stringify({{error:{{code:'NotFound',message:'item not found'}}}});}}return 'ok:'+verb;}}"}},{{"path":"notes.txt","content":"hello"}}]}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let good_content = structured_content(&good);
    assert_eq!(
        good_content["files"].as_array().map(Vec::len),
        Some(2),
        "batch write summaries: {good}"
    );
    assert!(
        good.contains("app_build_validate"),
        "batch write should route to validation: {good}"
    );

    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert_eq!(
        structured_content(&validate)["valid"],
        true,
        "batch-updated draft validates: {validate}"
    );
}

#[test]
fn app_build_validate_warns_on_ui_invoke_array_args() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"uiarg-demo","name":"UiArg Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    // The observed run-2 mistake: an args array as the second invoke argument.
    let ui_js = "async function send(text, refDate) {\n  return window.terrane.invoke('nl_view', [text, refDate]);\n}\nsend('x', 'y');\n";
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"ui.js","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(ui_js)
        ),
    )
    .unwrap();

    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validation = structured_content(&validate);
    assert_eq!(validation["valid"], true, "array-arg UI: {validate}");
    let warnings = validation["warnings"].to_string();
    assert!(
        warnings.contains("passes an array to invoke()")
            && warnings.contains("positional string args"),
        "array-arg UI should warn with the positional contract: {warnings}"
    );
}

#[test]
fn app_build_errors_carry_structured_recovery() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let missing = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"missing","method":"tools/call","params":{"name":"app_build_put_file","arguments":{"draftId":"draft-00000000000000000000000000000000","path":"main.js","content":"x"}}}"#,
    )
    .unwrap();
    assert!(missing.contains(r#""isError":true"#), "missing: {missing}");
    let content = structured_content(&missing);
    assert_eq!(content["type"], "build_error", "missing: {missing}");
    assert_eq!(
        content["nextToolCall"]["tool"], "app_build_list",
        "lost draftId should route to app_build_list: {missing}"
    );
}

#[test]
fn app_build_get_flags_unmodified_scaffold_files() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"pristine-demo","name":"Pristine Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    let get_all = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"all","method":"tools/call","params":{{"name":"app_build_get","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let all = structured_content(&get_all);
    assert!(
        all["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["unmodifiedScaffold"] == true),
        "fresh draft files are all pristine: {get_all}"
    );
    assert!(
        get_all.contains("Do not read files marked unmodifiedScaffold"),
        "summary should steer away from scaffold reads: {get_all}"
    );
    assert!(
        start.contains("Reply with a tool call only")
            && get_all.contains("Reply with a tool call only"),
        "start/get should carry the anti-prose nextModelAction: {start}"
    );

    let get_one = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"one","method":"tools/call","params":{{"name":"app_build_get","arguments":{{"draftId":{},"path":"style.css"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert!(
        get_one.contains("unmodified scaffold shell"),
        "pristine single-file get should carry the note: {get_one}"
    );

    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"main.js","content":"function handle(input){{if((input[0]||'')==='__actions__'){{return JSON.stringify({{actions:[]}});}}return 'ok';}}"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let after = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"after","method":"tools/call","params":{{"name":"app_build_get","arguments":{{"draftId":{},"path":"main.js"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert_eq!(
        structured_content(&after)["unmodifiedScaffold"],
        false,
        "edited file is no longer pristine: {after}"
    );
}

#[test]
fn app_build_validate_rejects_js_syntax_errors() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"syntax-demo","name":"Syntax Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    // A truncated backend — the run-5 DeepSeek Flash failure class.
    let truncated = "function handle(input) {\n  var verb = input[0] || \"\";\n  if (verb === \"__actions__\") { return JSON.stringify({actions: [\n";
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"main.js","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(truncated)
        ),
    )
    .unwrap();
    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validation = structured_content(&validate);
    assert_eq!(validation["valid"], false, "truncated backend: {validate}");
    assert!(
        validation["errors"]
            .to_string()
            .contains("JavaScript syntax error"),
        "truncated backend should fail with a syntax error: {validate}"
    );

    // Broken ui.js breaks the page — also an error.
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"fixmain","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"main.js","content":"function handle(input){{if((input[0]||'')==='__actions__'){{return JSON.stringify({{actions:[]}});}}return 'ok';}}"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"badui","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"ui.js","content":"const x = {{ oops: ;"}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validate_ui = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v2","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let validation_ui = structured_content(&validate_ui);
    assert_eq!(validation_ui["valid"], false, "broken ui.js: {validate_ui}");
    assert!(
        validation_ui["errors"].to_string().contains("ui.js"),
        "broken ui.js should be named: {validate_ui}"
    );
}

#[test]
fn ui_raw_invoke_fetch_warns() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"fetch-demo","name":"Fetch Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();
    let ui_js = "async function load() {\n  var res = await fetch('/apps/fetch-demo/invoke', { method: 'POST', body: JSON.stringify({ verb: 'list', args: [{}] }) });\n  return res.json();\n}\nload();\n";
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"ui.js","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(ui_js)
        ),
    )
    .unwrap();
    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    assert!(
        structured_content(&validate)["warnings"]
            .to_string()
            .contains("window.terrane.invoke"),
        "raw /invoke fetch should warn toward the bridge: {validate}"
    );
}

#[test]
fn ui_missing_element_id_warns() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let start = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":"start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"ids-demo","name":"Ids Demo","withUi":true}}}"#,
    )
    .unwrap();
    let draft_id = structured_content(&start)["draftId"]
        .as_str()
        .unwrap()
        .to_string();

    // The run-6 Kimi class: ui.js wires an id its own HTML never defines.
    let ui_js = "document.getElementById('nl-input-box').addEventListener('keydown', function () {});\ndocument.querySelector('#list').textContent = 'ok';\n";
    handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"put","method":"tools/call","params":{{"name":"app_build_put_file","arguments":{{"draftId":{},"path":"ui.js","content":{}}}}}}}"#,
            super::json_str(&draft_id),
            super::json_str(ui_js)
        ),
    )
    .unwrap();
    let validate = handle_json_rpc(
        &mut core,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"v","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":{}}}}}}}"#,
            super::json_str(&draft_id)
        ),
    )
    .unwrap();
    let warnings = structured_content(&validate)["warnings"].to_string();
    assert!(
        warnings.contains("nl-input-box") && warnings.contains("crash on load"),
        "missing element id should warn: {warnings}"
    );
    assert!(
        !warnings.contains("that no bundle HTML defines. If they are not created dynamically")
            || !warnings.contains("\"list\""),
        "ids present in index.html must not be flagged: {warnings}"
    );
}

#[test]
fn backend_without_actions_verb_warns() {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    super::check_js_backend_contract(
        "main.js",
        "function handle(input) { var verb = input[0] || \"\"; return verb; }",
        &mut errors,
        &mut warnings,
    );
    assert!(errors.is_empty(), "custom handle is valid: {errors:?}");
    assert!(
        warnings.iter().any(|w| w.contains("__actions__")),
        "missing __actions__ should warn: {warnings:?}"
    );
}

#[test]
fn stale_drafts_are_evicted_beyond_cap() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _restore = isolate_home(dir.path());

    let files = vec![super::InlineFile {
        path: "manifest.json".to_string(),
        content: r#"{"id":"gc-demo","name":"GC","runtime":"js","backend":"main.js"}"#.to_string(),
    }];
    for _ in 0..20 {
        super::create_build_draft("js_kv_notes", &files).unwrap();
    }
    let count = std::fs::read_dir(dir.path().join(".mcp-drafts"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("draft-"))
        })
        .count();
    assert_eq!(count, 16, "draft cap should evict the oldest drafts");
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
    let response = r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"x"}],"structuredContent":{"type":"permission_required","requestId":"local-demo-kv","app":"demo","appName":"Demo","missingResources":["kv","crdt"],"adminUrl":"http://127.0.0.1:8780/__terrane/admin/requests/local-demo-kv","requestStatus":"pending"},"isError":true}}"#;
    let info = super::permission_required_from_tool_response(response).expect("elicit info");
    assert_eq!(info.request_id, "local-demo-kv");
    assert_eq!(info.app, "demo");
    assert_eq!(info.app_name, "Demo");
    assert_eq!(
        info.missing_resources,
        vec!["kv".to_string(), "crdt".to_string()]
    );
    assert!(info.admin_url.ends_with("/local-demo-kv"));

    // An ordinary (non-permission) result yields nothing to elicit.
    let ok = r#"{"jsonrpc":"2.0","id":6,"result":{"content":[{"type":"text","text":"done"}],"isError":false}}"#;
    assert!(super::permission_required_from_tool_response(ok).is_none());

    // A dry-run preview is permission-shaped, but not an approvable pending request.
    let preview = r#"{"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"x"}],"structuredContent":{"type":"permission_required","requestId":"local-demo-kv","app":"demo","appName":"Demo","missingResources":["kv"],"adminUrl":"http://127.0.0.1:8780/__terrane/admin/requests/local-demo-kv","requestStatus":"preview"},"isError":true}}"#;
    assert!(super::permission_required_from_tool_response(preview).is_none());
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
    assert!(
        frame.contains(r#""method":"elicitation/create""#),
        "{frame}"
    );
    assert!(frame.contains(r#""id":"terrane-elicit-1""#), "{frame}");
    assert!(
        frame.contains("Demo") && frame.contains("kv, crdt"),
        "{frame}"
    );
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

    assert_eq!(
        super::elicitation_decision(accept_approve, id),
        Some(super::ElicitDecision::Approve)
    );
    assert_eq!(
        super::elicitation_decision(accept_deny, id),
        Some(super::ElicitDecision::Deny)
    );
    assert_eq!(
        super::elicitation_decision(decline, id),
        Some(super::ElicitDecision::Deny)
    );
    assert_eq!(
        super::elicitation_decision(cancel, id),
        Some(super::ElicitDecision::Deny)
    );
    assert_eq!(
        super::elicitation_decision(err, id),
        Some(super::ElicitDecision::Deny)
    );
    // A different id is not our response — keep waiting.
    assert_eq!(super::elicitation_decision(other_id, id), None);
}

#[test]
fn busy_error_replies_to_requests_but_ignores_notifications() {
    let request = r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#;
    let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let reply = super::busy_error(request).expect("busy error for a request");
    assert!(
        reply.contains(r#""id":9"#) && reply.contains("awaiting an elicitation"),
        "{reply}"
    );
    assert!(super::busy_error(notification).is_none());
    assert_eq!(super::parsed_method(request).as_deref(), Some("tools/list"));
}
