//! E2E for `terrane-web`: spawn the real binary on an ephemeral loopback port,
//! then drive the HTTP contract — health, catalog, UI serving (with the injected
//! invoke shim), the invoke round-trip, and the path-traversal guard.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tempfile::tempdir;
use terrane_core::Core;
use terrane_domain::Request;

fn app_source_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("apps/{name} exists"))
}

fn app_source(name: &str) -> String {
    app_source_path(name).to_str().unwrap().to_string()
}

fn install_from_source(core: &mut Core, id: &str, source: &Path) {
    core.dispatch(Request::new(
        "app.add",
        vec![
            id.into(),
            id.into(),
            "--source".into(),
            source.to_str().unwrap().to_string(),
        ],
    ))
    .unwrap();
}

fn install(core: &mut Core, id: &str) {
    core.dispatch(Request::new(
        "app.add",
        vec![id.into(), id.into(), "--source".into(), app_source(id)],
    ))
    .unwrap();
}

fn copy_dir(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Minimal blocking HTTP/1.0 client (Connection: close → read to EOF).
fn http(addr: &str, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    http_with_headers(addr, method, path, body, &[])
}

fn http_with_headers(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let mut req = format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n");
    for (field, value) in headers {
        req.push_str(field);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    if let Some(b) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status = raw
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// Spawn terrane-web on an ephemeral port; return (child, addr) once it's bound.
fn spawn_web(home: &std::path::Path) -> (Child, String) {
    spawn_web_with(home, "127.0.0.1:0", None)
}

fn spawn_web_with(home: &std::path::Path, bind: &str, token: Option<&str>) -> (Child, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terrane-web"));
    cmd.args(["--addr", bind])
        .env("TERRANE_HOME", home)
        .stderr(Stdio::piped())
        .stdout(Stdio::null());
    if let Some(token) = token {
        cmd.env("TERRANE_WEB_TOKEN", token);
    }
    let mut child = cmd.spawn().expect("spawn terrane-web");
    let stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stderr).lines();
    // First stderr line: "...serving <home> on http://127.0.0.1:PORT (auth: ...)"
    let line = lines.next().expect("server prints a startup line").unwrap();
    let addr = line
        .split("http://")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .expect("startup line has an address")
        .to_string();
    (child, addr)
}

#[test]
fn serves_catalog_ui_and_invoke_over_http() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo"); // has a UI
        install(&mut core, "bmi-calculator"); // React shell UI
        install(&mut core, "todo-cli-collaborate"); // crdt add/list
    }

    let (mut child, addr) = spawn_web(home);

    // healthz
    let (status, body) = http(&addr, "GET", "/healthz", None);
    assert_eq!(status, 200, "healthz body: {body}");
    assert!(body.contains("\"status\":\"ok\""), "healthz: {body}");

    // catalog
    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200);
    assert!(
        body.contains("todo-cli-collaborate")
            && body.contains("bmi-calculator")
            && body.contains("\"todo\""),
        "apps: {body}"
    );

    // Shell: wraps the app in host-owned navigation; the browser loads the
    // dynamic app list from `/apps`.
    let (status, body) = http(&addr, "GET", "/apps/todo/", None);
    assert_eq!(status, 200, "shell body: {body}");
    assert!(body.contains("Terrane"), "shell brand missing: {body}");
    assert!(
        body.contains("id=\"app-list\""),
        "dynamic app list mount missing: {body}"
    );
    assert!(
        body.contains("fetch(\"/apps\""),
        "catalog loader missing: {body}"
    );
    assert!(
        body.contains("id=\"app-frame\""),
        "app frame missing: {body}"
    );

    // UI frame: serves the app's index.html with the invoke shim injected.
    let (status, body) = http(&addr, "GET", "/apps/todo/__terrane/frame/", None);
    assert_eq!(status, 200, "ui body: {body}");
    assert!(body.contains("window.terrane"), "shim missing: {body}");
    assert!(body.contains("window.APP_ID"), "app id missing: {body}");
    assert!(body.contains("\"todo\""), "app id value missing: {body}");
    assert!(
        body.contains("__terrane/live-version"),
        "live reload hook missing: {body}"
    );

    // React UI frame: manifest.ui can opt into the host-provided React shell.
    let (status, body) = http(&addr, "GET", "/apps/bmi-calculator/__terrane/frame/", None);
    assert_eq!(status, 200, "react shell body: {body}");
    assert!(body.contains("window.terrane"), "shim missing: {body}");
    assert!(
        body.contains("__terrane/react/react.js")
            && body.contains("__terrane/react/react-dom.js")
            && body.contains("__terrane/frame/app.js"),
        "react shell scripts missing: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/bmi-calculator/__terrane/react/react.js",
        None,
    );
    assert_eq!(status, 200, "react runtime: {body}");
    assert!(
        body.contains("window.React") && body.contains("useState"),
        "react runtime missing API: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/bmi-calculator/__terrane/frame/app.js",
        None,
    );
    assert_eq!(status, 200, "react app js: {body}");
    assert!(body.contains("BMI Calculator"), "react app missing: {body}");

    let (status, body) = http(&addr, "GET", "/apps/todo/__terrane/live-version", None);
    assert_eq!(status, 200, "live version: {body}");
    assert!(body.contains("\"version\""), "live version: {body}");

    // invoke round-trip on the crdt app.
    let (status, body) = http(
        &addr,
        "POST",
        "/apps/todo-cli-collaborate/invoke",
        Some(r#"{"verb":"add","args":["buy milk"]}"#),
    );
    assert_eq!(status, 200, "invoke add: {body}");
    assert!(body.contains("added: buy milk"), "invoke add: {body}");

    let (_, body) = http(
        &addr,
        "POST",
        "/apps/todo-cli-collaborate/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
    );
    assert!(body.contains("buy milk"), "invoke list (read back): {body}");

    // invoke on a missing app → 404.
    let (status, _) = http(
        &addr,
        "POST",
        "/apps/ghost/invoke",
        Some(r#"{"verb":"x","args":[]}"#),
    );
    assert_eq!(status, 404);

    // MCP over HTTP uses the same list → discover → act semantics as stdio.
    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":11,"method":"initialize","params":{}}"#),
    );
    assert_eq!(status, 200, "mcp initialize: {body}");
    assert!(body.contains("\"serverInfo\""), "mcp initialize: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":12,"method":"tools/list"}"#),
    );
    assert_eq!(status, 200, "mcp tools/list: {body}");
    assert!(
        body.contains("list_apps") && body.contains("app_actions") && body.contains("invoke"),
        "mcp tools/list: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"app_actions","arguments":{"app":"todo-cli-collaborate"}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp app_actions: {body}");
    assert!(
        body.contains("actions") && body.contains("add") && body.contains("list"),
        "mcp app_actions: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"todo-cli-collaborate","verb":"add","args":["via mcp http"]}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp invoke add: {body}");
    assert!(
        body.contains("added: via mcp http"),
        "mcp invoke add: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"todo-cli-collaborate","verb":"list","args":[]}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp invoke list: {body}");
    assert!(body.contains("via mcp http"), "mcp invoke list: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
    );
    assert_eq!(status, 202, "mcp notification body: {body}");

    let (status, _) = http(&addr, "GET", "/mcp", None);
    assert_eq!(status, 405);

    let (status, _) = http_with_headers(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":16,"method":"ping"}"#),
        &[("Origin", "https://example.invalid")],
    );
    assert_eq!(status, 403);

    let (status, body) = http_with_headers(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":17,"method":"ping"}"#),
        &[("Origin", "http://localhost")],
    );
    assert_eq!(status, 200, "loopback origin ping: {body}");
    assert!(body.contains("\"id\":17"), "loopback origin ping: {body}");

    // path traversal is refused.
    let (status, _) = http(&addr, "GET", "/apps/todo/../../Cargo.toml", None);
    assert!(status == 403 || status == 404, "traversal status: {status}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn non_loopback_bind_requires_bearer_auth_for_mcp() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (mut child, bind_addr) = spawn_web_with(home, "0.0.0.0:0", Some("secret"));
    let connect_addr = bind_addr.replacen("0.0.0.0", "127.0.0.1", 1);

    let (status, _) = http(
        &connect_addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
    );
    assert_eq!(status, 401);

    let (status, body) = http_with_headers(
        &connect_addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#),
        &[("Authorization", "Bearer secret")],
    );
    assert_eq!(status, 200, "authorized ping: {body}");
    assert!(body.contains("\"id\":2"), "authorized ping: {body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn live_version_changes_when_bundle_file_changes() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let app_dir = dir.path().join("todo-source");
    copy_dir(&app_source_path("todo"), &app_dir);
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install_from_source(&mut core, "todo", &app_dir);
    }

    let (mut child, addr) = spawn_web(&home);

    let (status, first) = http(&addr, "GET", "/apps/todo/__terrane/live-version", None);
    assert_eq!(status, 200, "first live version: {first}");

    std::fs::write(
        app_dir.join("index.html"),
        "<!doctype html><title>Todo Reloaded</title><h1>Todo Reloaded</h1>",
    )
    .unwrap();

    let (status, second) = http(&addr, "GET", "/apps/todo/__terrane/live-version", None);
    assert_eq!(status, 200, "second live version: {second}");
    assert_ne!(
        first, second,
        "live version should change after app file edit"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// Conformance: the running web host serves *every* HTTP route the contract
/// (`terrane_api::host_contract`) declares. Premium runs the analogous black-box
/// check against its server to prove it's a superset.
#[test]
fn web_host_serves_every_declared_route() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo"); // has a UI, so the UI route resolves
    }
    let (mut child, addr) = spawn_web(home);

    for route in terrane_api::host_contract().http_routes {
        let path = route.path.replace("{id}", "todo");
        let (status, body) = if route.method == "POST" {
            http(&addr, "POST", &path, Some(r#"{"verb":"list","args":[]}"#))
        } else {
            http(&addr, "GET", &path, None)
        };
        assert!(
            status != 0 && status != 404,
            "declared route {} {path} not served (status {status}): {body}",
            route.method
        );
    }

    let _ = child.kill();
    let _ = child.wait();
}
