//! E2E for `terrane-web`: spawn the real binary on an ephemeral loopback port,
//! then drive the HTTP contract — health, catalog, UI serving (with the injected
//! invoke shim), the invoke round-trip, and the path-traversal guard.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use tempfile::tempdir;
use terrane_core::Core;
use terrane_domain::Request;

fn app_source(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("apps/{name} exists"))
        .to_str()
        .unwrap()
        .to_string()
}

fn install(core: &mut Core, id: &str) {
    core.dispatch(Request::new(
        "app.add",
        vec![id.into(), id.into(), "--source".into(), app_source(id)],
    ))
    .unwrap();
}

/// Minimal blocking HTTP/1.0 client (Connection: close → read to EOF).
fn http(addr: &str, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let mut req = format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n");
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_terrane-web"))
        .args(["--addr", "127.0.0.1:0"])
        .env("TERRANE_HOME", home)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn terrane-web");
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
    assert!(body.contains("todo-cli-collaborate") && body.contains("\"todo\""), "apps: {body}");

    // UI: serves the app's index.html with the invoke shim injected.
    let (status, body) = http(&addr, "GET", "/apps/todo/", None);
    assert_eq!(status, 200, "ui body: {body}");
    assert!(body.contains("window.terrane"), "shim missing: {body}");
    assert!(body.contains("APP_ID=\"todo\""), "app id missing: {body}");

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
    let (status, _) = http(&addr, "POST", "/apps/ghost/invoke", Some(r#"{"verb":"x","args":[]}"#));
    assert_eq!(status, 404);

    // path traversal is refused.
    let (status, _) = http(&addr, "GET", "/apps/todo/../../Cargo.toml", None);
    assert!(status == 403 || status == 404, "traversal status: {status}");

    let _ = child.kill();
    let _ = child.wait();
}
