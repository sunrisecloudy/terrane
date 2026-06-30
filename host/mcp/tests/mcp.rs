//! E2E for `terrane-mcp`: drive the real binary over stdin/stdout with JSON-RPC,
//! install `todo-cli-collaborate`, then ADD a todo through the MCP `invoke` tool
//! and READ IT BACK — the multi-app, select-then-act round-trip an MCP client
//! (e.g. Claude Code) performs.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tempfile::tempdir;
use terrane_core::Core;
use terrane_core::Request;

fn app_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/todo-cli-collaborate")
        .canonicalize()
        .expect("apps/todo-cli-collaborate exists")
        .to_str()
        .unwrap()
        .to_string()
}

fn send(stdin: &mut impl Write, json: &str) {
    stdin.write_all(json.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

fn read_line(out: &mut impl BufRead) -> String {
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    line
}

fn sqlite_value(path: &Path, app: &str, key: &str) -> Option<String> {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT value FROM kv_entries WHERE app = ?1 AND key = ?2",
            [app, key],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn structured_content(line: &str) -> Value {
    serde_json::from_str::<Value>(line).unwrap()["result"]["structuredContent"].clone()
}

#[test]
fn add_a_todo_through_mcp_and_read_it_back() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    // Install the app into this home (the MCP server will serve it).
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        core.dispatch(Request::new(
            "app.add",
            vec![
                "todo-cli-collaborate".into(),
                "Todo".into(),
                "--source".into(),
                app_source(),
            ],
        ))
        .unwrap();
        core.dispatch(Request::new(
            "auth.grant",
            vec![
                "user:local-owner".into(),
                "todo-cli-collaborate".into(),
                "crdt".into(),
            ],
        ))
        .unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_terrane-mcp"))
        .env("TERRANE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn terrane-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());

    // initialize handshake.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    let init = read_line(&mut out);
    assert!(init.contains("\"serverInfo\""), "init: {init}");
    assert!(init.contains("\"id\":1"), "init id echo: {init}");
    assert!(
        init.contains("\"resources\"") && init.contains("\"prompts\""),
        "init capabilities: {init}"
    );

    // initialized notification — no response expected.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    // tools/list advertises app tools plus capability-doc tools.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    let tools = read_line(&mut out);
    assert!(
        tools.contains("list_apps")
            && tools.contains("app_actions")
            && tools.contains("invoke")
            && tools.contains("workflows_list")
            && tools.contains("workflow_info")
            && tools.contains("app_scaffold")
            && tools.contains("app_bundle_validate")
            && tools.contains("app_register_inline")
            && tools.contains("app_register")
            && tools.contains("capabilities_list")
            && tools.contains("capability_info")
            && tools.contains("capability_query")
            && tools.contains("capability_command"),
        "tools/list: {tools}"
    );

    // resources/list exposes host-owned MCP docs, while templates point to
    // capability-owned docs.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"resources","method":"resources/list"}"#,
    );
    let resources = read_line(&mut out);
    assert!(
        resources.contains("terrane://docs/index")
            && resources.contains("terrane://docs/app-building"),
        "resources/list: {resources}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"templates","method":"resources/templates/list"}"#,
    );
    let templates = read_line(&mut out);
    assert!(
        templates.contains("terrane://capabilities/{namespace}")
            && templates.contains("terrane://workflows/{name}"),
        "resources/templates/list: {templates}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"doc","method":"resources/read","params":{"uri":"terrane://docs/app-building"}}"#,
    );
    let doc = read_line(&mut out);
    assert!(
        doc.contains("app_register_inline")
            && doc.contains("MCP App Building")
            && doc.contains("window.terrane.invoke")
            && doc.contains("app.remove")
            && doc.contains("kvGetOrNull")
            && doc.contains("Do not JSON-stringify")
            && doc.contains("complete files array"),
        "resources/read docs: {doc}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"cap-doc","method":"resources/read","params":{"uri":"terrane://capabilities/kv"}}"#,
    );
    let cap_doc = read_line(&mut out);
    assert!(
        cap_doc.contains("ctx.resource.kv") && cap_doc.contains("reserved"),
        "resources/read capability: {cap_doc}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"prompts","method":"prompts/list"}"#,
    );
    let prompts = read_line(&mut out);
    assert!(
        prompts.contains("make_js_kv_app") && prompts.contains("safe_capability_command"),
        "prompts/list: {prompts}"
    );
    assert!(
        !prompts.contains("make_js_multicap_app"),
        "eval prompt must not be served by MCP prompts/list: {prompts}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"prompt","method":"prompts/get","params":{"name":"make_js_kv_app","arguments":{"id":"prompt-notes","name":"Prompt Notes"}}}"#,
    );
    let prompt = read_line(&mut out);
    assert!(
        prompt.contains("app_register_inline") && prompt.contains("prompt-notes"),
        "prompts/get: {prompt}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"workflows-list","method":"tools/call","params":{"name":"workflows_list","arguments":{}}}"#,
    );
    let workflows = read_line(&mut out);
    assert!(
        workflows.contains("chooseByOutcome")
            && workflows.contains("multiple capabilities")
            && workflows.contains("withUi:true")
            && workflows.contains("kvGetOrNull")
            && workflows.contains("JSON array"),
        "workflows_list outcome chooser: {workflows}"
    );

    // list_apps → the app is selectable.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#,
    );
    let apps = read_line(&mut out);
    assert!(apps.contains("todo-cli-collaborate"), "list_apps: {apps}");
    assert!(
        apps.contains(r#""structuredContent""#),
        "list_apps structured content: {apps}"
    );

    // workflow_info gives weak models an exact recipe before low-level tools.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"workflow","method":"tools/call","params":{"name":"workflow_info","arguments":{"name":"make_js_kv_app"}}}"#,
    );
    let workflow = read_line(&mut out);
    assert!(
        workflow.contains("app_bundle_validate")
            && workflow.contains("app_register_inline")
            && workflow.contains("app_register")
            && workflow.contains("window.terrane.invoke")
            && workflow.contains("app.remove")
            && workflow.contains("kvGetOrNull")
            && workflow.contains("do not JSON-stringify")
            && workflow.contains(r#""structuredContent""#),
        "workflow_info: {workflow}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"ui-scaffold","method":"tools/call","params":{"name":"app_scaffold","arguments":{"id":"mcp-ui","name":"MCP UI","withUi":true}}}"#,
    );
    let ui_scaffold = read_line(&mut out);
    assert!(
        ui_scaffold.contains("ui.js")
            && ui_scaffold.contains("kvGetOrNull")
            && ui_scaffold.contains("complete files array")
            && ui_scaffold.contains("not a JSON string"),
        "app_scaffold UI guidance: {ui_scaffold}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-string-files","method":"tools/call","params":{"name":"app_register_inline","arguments":{"files":"[]","dryRun":true}}}"#,
    );
    let inline_string_files = read_line(&mut out);
    assert!(
        inline_string_files.contains(r#""isError":true"#)
            && inline_string_files.contains("do not JSON-stringify")
            && inline_string_files.contains("structuredContent.files"),
        "app_register_inline stringified files error: {inline_string_files}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"workflow-multi","method":"tools/call","params":{"name":"workflow_info","arguments":{"name":"make_js_multicap_app_no_filesystem"}}}"#,
    );
    let workflow_multi = read_line(&mut out);
    assert!(
        workflow_multi.contains("js_multicap_audit")
            && workflow_multi.contains("replica.peer")
            && workflow_multi.contains("relational_db")
            && workflow_multi.contains("pre-clear")
            && workflow_multi.contains("Do not count seed output")
            && workflow_multi.contains("post-clear")
            && workflow_multi.contains(r#""structuredContent""#),
        "workflow_info multicap: {workflow_multi}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-scaffold","method":"tools/call","params":{"name":"app_scaffold","arguments":{"id":"mcp-multicap","name":"MCP Multicap","kind":"js_multicap_audit","withUi":true}}}"#,
    );
    let multi_scaffold = read_line(&mut out);
    assert!(
        multi_scaffold.contains("js_multicap_audit")
            && multi_scaffold.contains("relational_db")
            && multi_scaffold.contains("clearKv")
            && multi_scaffold.contains("ui.js")
            && multi_scaffold.contains("window.terrane.invoke")
            && multi_scaffold.contains("kvGetOrNull")
            && multi_scaffold.contains("complete files array"),
        "app_scaffold multicap: {multi_scaffold}"
    );
    let multi_files = structured_content(&multi_scaffold)["files"].clone();

    let multi_dry_msg = json!({
        "jsonrpc": "2.0",
        "id": "multi-dry",
        "method": "tools/call",
        "params": {
            "name": "app_register_inline",
            "arguments": {
                "files": multi_files.clone(),
                "dryRun": true
            }
        }
    })
    .to_string();
    send(&mut stdin, &multi_dry_msg);
    let multi_dry = read_line(&mut out);
    assert!(
        multi_dry.contains(r#"\"dryRun\":true"#) && multi_dry.contains(r#""isError":false"#),
        "multicap app_register_inline dryRun: {multi_dry}"
    );

    let multi_commit_msg = json!({
        "jsonrpc": "2.0",
        "id": "multi-commit",
        "method": "tools/call",
        "params": {
            "name": "app_register_inline",
            "arguments": {
                "files": multi_files
            }
        }
    })
    .to_string();
    send(&mut stdin, &multi_commit_msg);
    let multi_commit = read_line(&mut out);
    assert!(
        multi_commit.contains(r#""isError":false"#) && multi_commit.contains("mcp-multicap"),
        "multicap app_register_inline commit: {multi_commit}"
    );
    for (id, namespace) in [
        ("multi-grant-kv", "kv"),
        ("multi-grant-crdt", "crdt"),
        ("multi-grant-rdb", "relational_db"),
    ] {
        send(
            &mut stdin,
            &format!(
                r#"{{"jsonrpc":"2.0","id":"{id}","method":"tools/call","params":{{"name":"capability_command","arguments":{{"name":"auth.grant","args":["user:local-owner","mcp-multicap","{namespace}"]}}}}}}"#
            ),
        );
        let grant = read_line(&mut out);
        assert!(
            grant.contains(r#""isError":false"#),
            "multicap auth.grant {namespace}: {grant}"
        );
    }

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"replica-help","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"replica.init","help":true}}}"#,
    );
    let replica_help = read_line(&mut out);
    assert!(
        replica_help.contains("replica.init") && replica_help.contains("replica.initialized"),
        "replica help: {replica_help}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"replica-init","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"replica.init"}}}"#,
    );
    let replica_init = read_line(&mut out);
    assert!(
        replica_init.contains(r#""isError":false"#),
        "replica init: {replica_init}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"replica-peer","method":"tools/call","params":{"name":"capability_query","arguments":{"capability":"replica","query":"peer","args":[]}}}"#,
    );
    let replica_peer = read_line(&mut out);
    let peer_value = structured_content(&replica_peer)["value"].clone();
    assert!(
        replica_peer.contains(r#""isError":false"#) && peer_value.as_u64().is_some(),
        "replica peer: {replica_peer}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-exists","method":"tools/call","params":{"name":"capability_query","arguments":{"capability":"app","query":"exists","args":["mcp-multicap"]}}}"#,
    );
    let multi_exists = read_line(&mut out);
    assert!(
        multi_exists.contains(r#"\"value\":true"#) && multi_exists.contains(r#""isError":false"#),
        "multicap app.exists: {multi_exists}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-actions","method":"tools/call","params":{"name":"app_actions","arguments":{"app":"mcp-multicap"}}}"#,
    );
    let multi_actions = read_line(&mut out);
    assert!(
        multi_actions.contains("seed")
            && multi_actions.contains("summary")
            && multi_actions.contains("clearKv")
            && multi_actions.contains("relational_db"),
        "multicap app_actions: {multi_actions}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-seed","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-multicap","verb":"seed","args":["mcp multicap test"]}}}"#,
    );
    let multi_seed = read_line(&mut out);
    assert!(
        multi_seed.contains("mcp multicap test")
            && multi_seed.contains("relational")
            && multi_seed.contains("crdt")
            && multi_seed.contains("kv"),
        "multicap seed: {multi_seed}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-summary","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-multicap","verb":"summary","args":[]}}}"#,
    );
    let multi_summary = read_line(&mut out);
    assert!(
        multi_summary.contains("mcp multicap test") && multi_summary.contains("Ada"),
        "multicap summary: {multi_summary}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-clear","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-multicap","verb":"clearKv","args":[]}}}"#,
    );
    let multi_clear = read_line(&mut out);
    assert!(
        multi_clear.contains(r#"\"lastNote\":null"#)
            && multi_clear.contains(r#"\"theme\":null"#)
            && multi_clear.contains("Ada"),
        "multicap clearKv: {multi_clear}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"multi-summary-after-clear","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-multicap","verb":"summary","args":[]}}}"#,
    );
    let multi_summary_after_clear = read_line(&mut out);
    assert!(
        multi_summary_after_clear.contains(r#"\"lastNote\":null"#)
            && multi_summary_after_clear.contains("mcp multicap test")
            && multi_summary_after_clear.contains("Ada"),
        "multicap summary after clear: {multi_summary_after_clear}"
    );

    // app_register_inline lets locked-down MCP clients create apps without
    // source reads or shell/file listing tools.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-dry","method":"tools/call","params":{"name":"app_register_inline","arguments":{"dryRun":true,"files":[{"path":"manifest.json","content":"{\"id\":\"mcp-inline\",\"name\":\"MCP Inline\",\"runtime\":\"js\",\"backend\":\"main.js\",\"resources\":[\"kv\"]}"},{"path":"main.js","content":"function handle(input){var verb=input[0]||'';var kv=ctx.resource.kv;if(verb==='__actions__'){return JSON.stringify({app:'mcp-inline',actions:[{verb:'write',args:[{name:'text',required:true}],returns:'stored'},{verb:'read',args:[],returns:'text'}]});}if(verb==='write'){kv.set('note',input.slice(1).join(' '));return 'stored';}if(verb==='read'){return kv.get('note')||'(empty)';}return 'unknown';}"}]}}}"#,
    );
    let inline_dry = read_line(&mut out);
    assert!(
        inline_dry.contains(r#"\"dryRun\":true"#) && inline_dry.contains(r#""isError":false"#),
        "app_register_inline dryRun: {inline_dry}"
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-commit","method":"tools/call","params":{"name":"app_register_inline","arguments":{"files":[{"path":"manifest.json","content":"{\"id\":\"mcp-inline\",\"name\":\"MCP Inline\",\"runtime\":\"js\",\"backend\":\"main.js\",\"resources\":[\"kv\"]}"},{"path":"main.js","content":"function handle(input){var verb=input[0]||'';var kv=ctx.resource.kv;if(verb==='__actions__'){return JSON.stringify({app:'mcp-inline',actions:[{verb:'write',args:[{name:'text',required:true}],returns:'stored'},{verb:'read',args:[],returns:'text'}]});}if(verb==='write'){kv.set('note',input.slice(1).join(' '));return 'stored';}if(verb==='read'){return kv.get('note')||'(empty)';}return 'unknown';}"}]}}}"#,
    );
    let inline_commit = read_line(&mut out);
    assert!(
        inline_commit.contains(r#""isError":false"#) && inline_commit.contains("mcp-inline"),
        "app_register_inline commit: {inline_commit}"
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-grant-kv","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"auth.grant","args":["user:local-owner","mcp-inline","kv"]}}}"#,
    );
    let inline_grant = read_line(&mut out);
    assert!(
        inline_grant.contains(r#""isError":false"#),
        "inline auth.grant: {inline_grant}"
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-write","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-inline","verb":"write","args":["hello inline"]}}}"#,
    );
    let inline_write = read_line(&mut out);
    assert!(
        inline_write.contains("stored"),
        "inline write: {inline_write}"
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"inline-read","method":"tools/call","params":{"name":"invoke","arguments":{"app":"mcp-inline","verb":"read","args":[]}}}"#,
    );
    let inline_read = read_line(&mut out);
    assert!(
        inline_read.contains("hello inline"),
        "inline read: {inline_read}"
    );

    // capability_query → read-only core query over stdio transport.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"query","method":"tools/call","params":{"name":"capability_query","arguments":{"capability":"app","query":"exists","args":["todo-cli-collaborate"]}}}"#,
    );
    let query = read_line(&mut out);
    assert!(
        query.contains(r#"\"value\":true"#) && query.contains(r#""isError":false"#),
        "capability_query: {query}"
    );

    // capability_command dryRun validates without committing.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"dry","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"app.add","args":["mcp-dry","MCP Dry"],"dryRun":true}}}"#,
    );
    let dry = read_line(&mut out);
    assert!(
        dry.contains(r#"\"dryRun\":true"#) && dry.contains(r#""isError":false"#),
        "capability_command dryRun: {dry}"
    );

    // capability_command → real writes use the default SQLite KV projection at
    // <TERRANE_HOME>/terrane.db.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"kv-app","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"app.add","args":["mcp-kv-default","MCP KV Default"]}}}"#,
    );
    let kv_app = read_line(&mut out);
    assert!(
        kv_app.contains(r#""isError":false"#),
        "capability_command app.add: {kv_app}"
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"kv-set","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"kv.set","args":["mcp-kv-default","note","stored in terrane.db"]}}}"#,
    );
    let kv_set = read_line(&mut out);
    assert!(
        kv_set.contains(r#""isError":false"#),
        "capability_command kv.set: {kv_set}"
    );
    let sqlite = home.join("terrane.db");
    assert!(sqlite.is_file(), "default KV sqlite file should exist");
    assert_eq!(
        sqlite_value(&sqlite, "mcp-kv-default", "note"),
        Some("stored in terrane.db".into())
    );

    // app_actions → the app describes its verbs programmatically (from __actions__).
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"act","method":"tools/call","params":{"name":"app_actions","arguments":{"app":"todo-cli-collaborate"}}}"#,
    );
    let acts = read_line(&mut out);
    // The app's JSON is nested (and escaped) inside the MCP result text.
    assert!(
        acts.contains("actions")
            && acts.contains("add")
            && acts.contains("list")
            && acts.contains("done"),
        "app_actions: {acts}"
    );
    assert!(acts.contains("\"id\":\"act\""), "string id echoed: {acts}");

    // invoke add — take action on the app.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"todo-cli-collaborate","verb":"add","args":["buy milk"]}}}"#,
    );
    let added = read_line(&mut out);
    assert!(added.contains("added: buy milk"), "invoke add: {added}");
    assert!(
        added.contains("\"isError\":false"),
        "invoke add not error: {added}"
    );

    // invoke list — READ IT BACK.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"todo-cli-collaborate","verb":"list","args":[]}}}"#,
    );
    let listed = read_line(&mut out);
    assert!(
        listed.contains("buy milk"),
        "invoke list (read back): {listed}"
    );

    // Unknown tool is a tool error, not a crash.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
    );
    let unknown = read_line(&mut out);
    assert!(unknown.contains("unknown tool"), "unknown tool: {unknown}");

    // --- regression: structural id parsing (the extract_id substring bug) ---

    // A tool ARGUMENT literally equal to "id" must NOT swallow the response: the
    // old substring scan matched the "id" inside args:["id"] and dropped the reply.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"todo-cli-collaborate","verb":"add","args":["id"]}}}"#,
    );
    let with_id_arg = read_line(&mut out);
    assert!(
        with_id_arg.contains("\"id\":7"),
        "top-level id echoed, not the arg: {with_id_arg}"
    );
    assert!(
        with_id_arg.contains("added: id"),
        "added the literal 'id': {with_id_arg}"
    );

    // A nested "id" before the top-level id must not be echoed in its place.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"ping","params":{"item":{"id":555}},"id":8}"#,
    );
    let nested = read_line(&mut out);
    assert!(
        nested.contains("\"id\":8"),
        "echoed top-level id 8, not nested 555: {nested}"
    );
    assert!(!nested.contains("555"), "nested id leaked: {nested}");

    // A string id round-trips verbatim.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"abc-9","method":"tools/list"}"#,
    );
    let strid = read_line(&mut out);
    assert!(
        strid.contains("\"id\":\"abc-9\""),
        "string id echoed: {strid}"
    );

    // EOF → the server exits cleanly.
    drop(stdin);
    let mut rest = String::new();
    let _ = out.read_to_string(&mut rest);
    assert!(child.wait().unwrap().success());
}
