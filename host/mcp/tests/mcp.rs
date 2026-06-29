//! E2E for `terrane-mcp`: drive the real binary over stdin/stdout with JSON-RPC,
//! install `todo-cli-collaborate`, then ADD a todo through the MCP `invoke` tool
//! and READ IT BACK — the multi-app, select-then-act round-trip an MCP client
//! (e.g. Claude Code) performs.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusqlite::{Connection, OptionalExtension};
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
            && tools.contains("app_register")
            && tools.contains("capabilities_list")
            && tools.contains("capability_info")
            && tools.contains("capability_query")
            && tools.contains("capability_command"),
        "tools/list: {tools}"
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
            && workflow.contains("app_register")
            && workflow.contains(r#""structuredContent""#),
        "workflow_info: {workflow}"
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
