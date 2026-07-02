//! E2E for `terrane-web`: spawn the real binary on an ephemeral loopback port,
//! then drive the HTTP contract — health, catalog, UI serving (with the injected
//! invoke shim), the invoke round-trip, and the path-traversal guard.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tempfile::tempdir;
use terrane_core::Core;
use terrane_core::Request;

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

fn install_named(core: &mut Core, id: &str, name: &str) {
    core.dispatch(Request::new(
        "app.add",
        vec![id.into(), name.into(), "--source".into(), app_source(id)],
    ))
    .unwrap();
}

fn grant_resource(core: &mut Core, app: &str, namespace: &str) {
    core.dispatch(Request::trusted_host(
        "auth.grant",
        vec!["user:local-owner".into(), app.into(), namespace.into()],
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

fn write_built_react_fixture(root: &Path) -> PathBuf {
    let app_dir = root.join("built-react");
    std::fs::create_dir_all(app_dir.join("dist/assets")).unwrap();
    std::fs::write(
        app_dir.join("manifest.json"),
        r#"{
  "id": "built-react",
  "name": "Built React",
  "version":"0.1.0","runtime":"js","backend":"main.js",
  "ui": "dist/index.html",
  "resources": []
}
"#,
    )
    .unwrap();
    std::fs::write(app_dir.join("main.js"), "export default async () => '';\n").unwrap();
    std::fs::write(
        app_dir.join("dist/index.html"),
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>Built React Fixture</title>
  <link rel="stylesheet" href="./assets/index.css">
</head>
<body>
  <div id="root"></div>
  <script type="module" src="./assets/index.js"></script>
</body>
</html>
"#,
    )
    .unwrap();
    std::fs::write(
        app_dir.join("dist/assets/index.js"),
        "document.body.dataset.builtReact = 'loaded';\n",
    )
    .unwrap();
    std::fs::write(
        app_dir.join("dist/assets/index.css"),
        "body { font-family: system-ui; }\n",
    )
    .unwrap();
    app_dir
}

/// Minimal blocking HTTP/1.0 client (Connection: close → read to EOF).
fn http(addr: &str, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    if needs_admin_header(path) {
        http_with_headers(
            addr,
            method,
            path,
            body,
            &[("X-Terrane-Admin", "local-admin")],
        )
    } else {
        http_with_headers(addr, method, path, body, &[])
    }
}

fn http_without_admin(addr: &str, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    http_with_headers(addr, method, path, body, &[])
}

fn needs_admin_header(path: &str) -> bool {
    path.starts_with("/__terrane/admin/")
        || matches!(
            path,
            "/__terrane/admin/session"
                | "/__terrane/admin/apps"
                | "/__terrane/admin/grants"
                | "/__terrane/admin/agents"
                | "/__terrane/admin/audit"
                | "/__terrane/admin/requests"
        )
}

fn http_with_headers(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let (status, _headers, body) = http_raw_with_headers(addr, method, path, body, headers);
    (status, body)
}

fn http_raw_with_headers(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (u16, String, String) {
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
    let mut parts = raw.splitn(2, "\r\n\r\n");
    let headers = parts.next().unwrap_or("").to_string();
    let body = parts.next().unwrap_or("").to_string();
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, headers, body)
}

fn preview_body(files: &[(&str, &str)]) -> String {
    let files = files
        .iter()
        .map(|(path, content)| {
            format!(
                r#"{{"path":"{}","content":"{}"}}"#,
                json_escape(path),
                json_escape(content)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"files":[{files}]}}"#)
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn json_string_field(body: &str, field: &str) -> String {
    let key = format!(r#""{field}":""#);
    let start = body
        .find(&key)
        .unwrap_or_else(|| panic!("missing field {field} in {body}"))
        + key.len();
    let rest = &body[start..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("unterminated field {field} in {body}"));
    rest[..end].to_string()
}

/// A spawned server that dies with the test — panicking assertions must not
/// leak `terrane-web` processes (which hold home locks) on the machine.
struct WebServer(Child);

impl WebServer {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }

    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.0.wait()
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn terrane-web on an ephemeral port; return (server, addr) once it's bound.
fn spawn_web(home: &std::path::Path) -> (WebServer, String) {
    spawn_web_with(home, "127.0.0.1:0", None)
}

fn spawn_web_with(home: &std::path::Path, bind: &str, token: Option<&str>) -> (WebServer, String) {
    spawn_web_full(home, bind, token, &[], &[])
}

/// Spawn with `--apps <dir>` dev scanning enabled.
fn spawn_web_dev(home: &std::path::Path, apps_dir: &std::path::Path) -> (WebServer, String) {
    spawn_web_full(
        home,
        "127.0.0.1:0",
        None,
        &["--apps", apps_dir.to_str().unwrap()],
        &[],
    )
}

fn spawn_web_full(
    home: &std::path::Path,
    bind: &str,
    token: Option<&str>,
    extra_args: &[&str],
    envs: &[(&str, String)],
) -> (WebServer, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terrane-web"));
    cmd.args(["--addr", bind])
        .args(extra_args)
        .env("TERRANE_HOME", home)
        .stderr(Stdio::piped())
        .stdout(Stdio::null());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    if let Some(token) = token {
        cmd.env("TERRANE_WEB_TOKEN", token);
    }
    let mut child = cmd.spawn().expect("spawn terrane-web");
    let stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stderr).lines();
    let mut seen = Vec::new();
    for line in lines.by_ref().take(20) {
        let line = line.expect("server startup line is readable");
        if let Some(addr) = line
            .split("http://")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
        {
            return (WebServer(child), addr.to_string());
        }
        seen.push(line);
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("server did not print a startup address; saw {seen:?}");
}

#[test]
fn creates_serves_and_invokes_ephemeral_preview_without_catalog_entry() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (mut child, addr) = spawn_web(home);

    let create = preview_body(&[
        (
            "manifest.json",
            r#"{"id":"hello-preview","name":"Hello Preview","runtime":"js","backend":"main.js","ui":"index.html","resources":[]}"#,
        ),
        (
            "main.js",
            r#"var actions = {
  hello: {
    summary: "Return a greeting.",
    args: [],
    returns: "a greeting line.",
    run: function () {
      return "Hello from Preview";
    }
  }
};
"#,
        ),
        (
            "index.html",
            r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <link rel="stylesheet" href="style.css">
</head>
<body><h1>Hello Preview</h1></body>
</html>
"#,
        ),
        ("style.css", "body { color: rgb(1 2 3); }\n"),
    ]);

    let (status, body) = http(&addr, "POST", "/__terrane/previews", Some(&create));
    assert_eq!(status, 200, "create preview: {body}");
    let preview_id = json_string_field(&body, "id");
    let frame_url = json_string_field(&body, "frameUrl");
    assert!(
        frame_url == format!("/__terrane/previews/{preview_id}/frame/"),
        "frameUrl: {body}"
    );

    let (status, body) = http(&addr, "GET", &frame_url, None);
    assert_eq!(status, 200, "preview frame: {body}");
    assert!(
        body.contains("window.terrane"),
        "preview shim missing: {body}"
    );
    assert!(
        body.contains(&format!("/__terrane/previews/{preview_id}/invoke")),
        "preview invoke route missing: {body}"
    );
    assert!(
        !body.contains(&format!("/apps/{preview_id}/invoke")),
        "preview frame should not use installed-app invoke route: {body}"
    );
    assert!(
        body.contains("var previewUrl = null;"),
        "preview frames should disable nested preview creation: {body}"
    );

    let (status, headers, body) = http_raw_with_headers(
        &addr,
        "GET",
        &format!("/__terrane/previews/{preview_id}/frame/style.css"),
        None,
        &[],
    );
    assert_eq!(status, 200, "preview style: {body}");
    assert!(body.contains("rgb(1 2 3)"), "preview style: {body}");
    // Preview frames are sandboxed (opaque origin) like app frames, so ES
    // module assets need CORS headers to load at all.
    assert!(
        headers
            .lines()
            .any(|l| l.to_ascii_lowercase().starts_with("access-control-allow-origin:")),
        "preview asset missing CORS header for the sandboxed frame: {headers}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/previews/{preview_id}/invoke"),
        Some(r#"{"verb":"hello","args":[]}"#),
    );
    assert_eq!(status, 200, "preview invoke: {body}");
    assert!(
        body.contains("Hello from Preview"),
        "preview invoke output: {body}"
    );

    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200, "apps after preview: {body}");
    assert!(
        !body.contains(&preview_id) && !body.contains("hello-preview"),
        "preview leaked into /apps: {body}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn preview_with_resources_requires_admin_review_before_runtime_access() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo");
        install(&mut core, "bmi-calculator");
    }
    let (mut child, addr) = spawn_web(home);

    let create = preview_body(&[
        (
            "manifest.json",
            r#"{"id":"todo","name":"Todo Preview","runtime":"js","backend":"main.js","ui":"index.html","resources":["kv"]}"#,
        ),
        (
            "main.js",
            r#"var kv = ctx.resource.kv;
function handle(input) {
  if (input[0] === "set") { kv.set(input[1], input[2]); return "ok"; }
  if (input[0] === "get") { return kv.get(input[1]) || ""; }
  return "?";
}
"#,
        ),
        (
            "index.html",
            "<!doctype html><html><body>Todo Preview</body></html>",
        ),
    ]);

    let (status, body) = http(&addr, "POST", "/__terrane/previews", Some(&create));
    assert_eq!(status, 200, "create preview: {body}");
    let preview_id = json_string_field(&body, "id");

    let (status, body) = http(&addr, "GET", "/__terrane/admin/requests", None);
    assert_eq!(status, 200, "preview requests: {body}");
    assert!(
        body.contains(&preview_id)
            && body.contains(r#""source":"preview""#)
            && body.contains(r#""status":"pending""#),
        "preview request should be listed: {body}"
    );
    let request_id = json_string_field(&body, "requestId");

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/previews/{preview_id}/invoke"),
        Some(r#"{"verb":"set","args":["answer","42"]}"#),
    );
    assert_eq!(status, 403, "preview invoke should need approval: {body}");
    assert!(
        body.contains("permission_required")
            && body.contains(r#""source":"preview""#)
            && body.contains(&request_id),
        "preview permission body: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/approve"),
        Some(r#"{"reason":"ok"}"#),
    );
    assert_eq!(status, 200, "preview approve: {body}");
    assert!(
        body.contains(r#""status":"approved""#),
        "preview approve: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/previews/{preview_id}/invoke"),
        Some(r#"{"verb":"set","args":["answer","42"]}"#),
    );
    assert_eq!(status, 200, "preview invoke after approve: {body}");
    assert!(body.contains("ok"), "preview invoke after approve: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/promote"),
        Some(r#"{"reason":"wrong target","app":"bmi-calculator"}"#),
    );
    assert_eq!(
        status, 400,
        "mismatched preview promote should fail: {body}"
    );
    assert!(
        body.contains("does not match preview app"),
        "mismatched preview promote body: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/promote"),
        Some(r#"{"reason":"promote","app":"todo"}"#),
    );
    assert_eq!(status, 200, "preview promote: {body}");
    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/promote"),
        Some(r#"{"reason":"repeat"}"#),
    );
    assert_eq!(
        status, 200,
        "repeat preview promote should be idempotent: {body}"
    );
    let (status, body) = http(&addr, "GET", "/__terrane/admin/grants", None);
    assert_eq!(status, 200, "grants after preview promote: {body}");
    assert!(
        body.contains(r#""app":"todo""#) && body.contains(r#""namespace":"kv""#),
        "preview promotion should write installed-app grant: {body}"
    );
    assert!(
        !body.contains(r#""app":"bmi-calculator""#),
        "mismatched preview promotion must not grant another app: {body}"
    );

    let (status, body) = http(
        &addr,
        "DELETE",
        &format!("/__terrane/previews/{preview_id}"),
        None,
    );
    assert_eq!(status, 204, "preview destroy: {body}");
    let (status, body) = http(&addr, "GET", "/__terrane/admin/requests", None);
    assert_eq!(status, 200, "requests after destroy: {body}");
    assert!(
        !body.contains(&request_id),
        "destroy should clear preview request/grant: {body}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A fake `codex` CLI: writes a valid app bundle to the `--output-last-message`
/// file after a short delay, so the e2e proves the background job + status
/// polling flow without a real agent.
fn write_fake_codex(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("fake-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let manifest = r#"{\"id\":\"bg-demo\",\"name\":\"BG Demo\",\"version\":\"0.1.0\",\"runtime\":\"js\",\"backend\":\"main.js\",\"ui\":\"index.html\",\"resources\":[]}"#;
    let script = format!(
        r#"#!/bin/sh
out=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$a"; fi
  prev="$a"
done
sleep 1
cat > "$out" <<'JSON'
{{"files":[{{"path":"manifest.json","content":"{manifest}"}},{{"path":"main.js","content":"function handle(input) {{ return \"ok\"; }}"}},{{"path":"index.html","content":"<!doctype html><html><body>bg demo</body></html>"}}]}}
JSON
exit 0
"#
    );
    let path = bin_dir.join("codex");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    bin_dir
}

#[test]
fn builder_generate_runs_in_background_and_status_reports_the_draft() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin_dir = write_fake_codex(dir.path());
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (mut child, addr) = spawn_web_full(
        &home,
        "127.0.0.1:0",
        None,
        &[],
        &[("PATH", path_env)],
    );

    // Start returns immediately with a running job, not the draft.
    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/builder/generate",
        Some(r#"{"id":"bg-demo","name":"BG Demo","prompt":"make a demo","harness":"codex"}"#),
    );
    assert_eq!(status, 200, "generate start: {body}");
    assert!(
        body.contains(r#""status":"running""#) && body.contains("bg-demo"),
        "start should report a running job: {body}"
    );

    // The loop stays free while the harness runs: other routes answer.
    let (status, _) = http(&addr, "GET", "/healthz", None);
    assert_eq!(status, 200, "server should serve while generating");

    // Poll until the job commits; the fake harness sleeps ~1s.
    let mut draft = String::new();
    for _ in 0..40 {
        let (status, body) = http(
            &addr,
            "POST",
            "/__terrane/builder/status",
            Some(r#"{"id":"bg-demo"}"#),
        );
        assert_eq!(status, 200, "status poll: {body}");
        if !body.contains(r#""status":"running""#) {
            draft = body;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(
        draft.contains("manifest.json") && draft.contains("bg-demo"),
        "final status should be the committed draft: {draft}"
    );

    // Polling again after completion still serves the draft from state.
    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/builder/status",
        Some(r#"{"id":"bg-demo"}"#),
    );
    assert_eq!(status, 200, "post-completion status: {body}");
    assert!(body.contains("manifest.json"), "draft from state: {body}");

    // Unknown ids are a 404, not a hang.
    let (status, _) = http(
        &addr,
        "POST",
        "/__terrane/builder/status",
        Some(r#"{"id":"ghost"}"#),
    );
    assert_eq!(status, 404, "unknown draft should 404");

    let _ = child.kill();
    let _ = child.wait();

    // The committed records replay: reopen the log and check the draft.
    let core = Core::open(home.join("log.bin")).unwrap();
    assert!(
        core.state().builder.drafts.contains_key("bg-demo"),
        "draft persisted to the log"
    );
    assert!(core.replay_matches().unwrap(), "replay identity holds");
}

#[test]
fn builder_generate_route_rejects_invalid_request_before_harness() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (mut child, addr) = spawn_web(home);

    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/builder/generate",
        Some(r#"{"id":"bad/path","name":"Demo","prompt":"make a greeting app","harness":"codex"}"#),
    );
    assert_eq!(status, 500, "builder generate should reject early: {body}");
    assert!(
        body.contains("unsafe") && body.contains("bad/path"),
        "builder generate error should come from core validation: {body}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

fn write_dev_app(dir: &Path, id: &str, name: &str) -> PathBuf {
    let app = dir.join(id);
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("manifest.json"),
        format!(
            r#"{{ "id": "{id}", "name": "{name}", "runtime": "js", "backend": "main.js", "ui": "index.html", "resources": [] }}"#
        ),
    )
    .unwrap();
    std::fs::write(
        app.join("main.js"),
        "function handle(input) { return \"pong:\" + input[0]; }\n",
    )
    .unwrap();
    std::fs::write(
        app.join("index.html"),
        format!(
            "<!doctype html><html><head><title>{name}</title></head><body data-dev-app=\"{id}\"></body></html>"
        ),
    )
    .unwrap();
    app
}

#[test]
fn dev_apps_dir_scans_serves_and_lazily_catalogs() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let apps_dir = dir.path().join("bundles");
    std::fs::create_dir_all(&apps_dir).unwrap();
    write_dev_app(&apps_dir, "devdemo", "Dev Demo");

    let (mut child, addr) = spawn_web_dev(&home, &apps_dir);

    // Scanned into the catalog listing without an `app add`.
    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200, "apps: {body}");
    assert!(
        body.contains(r#""id":"devdemo""#)
            && body.contains("Dev Demo")
            && body.contains(r#""has_ui":true"#),
        "dev app missing from catalog: {body}"
    );

    // Shell, frame, and live-version all resolve the dev bundle source.
    let (status, body) = http(&addr, "GET", "/apps/devdemo/", None);
    assert_eq!(status, 200, "dev shell: {body}");
    let (status, body) = http(&addr, "GET", "/apps/devdemo/__terrane/frame/", None);
    assert_eq!(status, 200, "dev frame: {body}");
    assert!(
        body.contains("data-dev-app=\"devdemo\""),
        "dev frame body: {body}"
    );
    let (status, body) = http(&addr, "GET", "/apps/devdemo/__terrane/live-version", None);
    assert_eq!(status, 200, "dev live-version: {body}");
    assert!(body.contains("\"version\""), "dev live-version body: {body}");

    // First invoke lazily catalogs the dev app, then runs its backend.
    let (status, body) = http(
        &addr,
        "POST",
        "/apps/devdemo/invoke",
        Some(r#"{"verb":"ping","args":[]}"#),
    );
    assert_eq!(status, 200, "dev invoke: {body}");
    assert!(body.contains("pong:ping"), "dev invoke output: {body}");

    // A bundle dropped in AFTER startup appears on the next catalog fetch and
    // is servable immediately — no restart, no install.
    write_dev_app(&apps_dir, "late-arrival", "Late Arrival");
    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200);
    assert!(
        body.contains(r#""id":"late-arrival""#),
        "late dev app missing: {body}"
    );
    let (status, body) = http(&addr, "GET", "/apps/late-arrival/__terrane/frame/", None);
    assert_eq!(status, 200, "late dev frame: {body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn serves_home_landing_page_at_root() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (mut child, addr) = spawn_web(home);

    // Landing page: the shared terrane-host home page, configured for the web
    // host — catalog fetched from `/apps`, cards linking into the `/apps/{id}/`
    // shell, admin console in the footer.
    let (status, body) = http(&addr, "GET", "/", None);
    assert_eq!(status, 200, "home body: {body}");
    assert!(body.contains("<h1>Terrane</h1>"), "brand missing: {body}");
    assert!(
        body.contains("id=\"home-app-list\""),
        "dynamic app list mount missing: {body}"
    );
    assert!(
        body.contains(r#""catalogUrl":"/apps""#),
        "catalog url config missing: {body}"
    );
    assert!(
        body.contains(r#""appHref":"/apps/{id}/""#),
        "app link template missing: {body}"
    );
    assert!(
        body.contains(r#""adminHref":"/__terrane/admin""#) && body.contains("home-admin-link"),
        "admin console link missing: {body}"
    );
    assert!(
        body.contains("fetch(String(config.catalogUrl)"),
        "catalog loader missing: {body}"
    );
    assert!(
        body.contains(r#""catalogPollMs":3000"#),
        "live-reload catalog polling missing from landing page: {body}"
    );

    // The root route stays exact: unknown top-level paths still 404.
    let (status, _body) = http(&addr, "GET", "/no-such-page", None);
    assert_eq!(status, 404, "unknown top-level path should stay 404");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn serves_bmi_calculator_shell_frame_assets_and_backend() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install_named(&mut core, "bmi-calculator", "BMI Calculator");
    }

    let (mut child, addr) = spawn_web(home);

    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200, "apps: {body}");
    assert!(
        body.contains(r#""id":"bmi-calculator""#) && body.contains("BMI Calculator"),
        "bmi calculator missing from catalog: {body}"
    );

    let (status, body) = http(&addr, "GET", "/apps/bmi-calculator/", None);
    assert_eq!(status, 200, "bmi shell: {body}");
    assert!(body.contains("Terrane"), "shell brand missing: {body}");
    assert!(
        body.contains("id=\"desktop-info-button\"")
            && body.contains("Live code editing uses the Terrane desktop app"),
        "desktop editing info missing: {body}"
    );
    assert!(
        body.contains("id=\"app-frame\""),
        "bmi shell iframe missing: {body}"
    );
    assert!(
        body.contains(
            "sandbox=\"allow-scripts allow-forms allow-modals allow-popups allow-downloads\""
        ),
        "bmi shell iframe should be sandboxed away from admin origin: {body}"
    );
    assert!(
        body.contains("/apps/\" + encodeURIComponent(currentId) + \"/__terrane/frame/"),
        "bmi shell frame loader missing: {body}"
    );

    let (status, body) = http(&addr, "GET", "/apps/bmi-calculator/__terrane/frame/", None);
    assert_eq!(status, 200, "bmi frame: {body}");
    assert!(body.contains("window.terrane"), "bmi shim missing: {body}");
    assert!(
        body.contains("\"bmi-calculator\""),
        "bmi app id missing from shim: {body}"
    );
    assert!(
        body.contains("assets/modules/src/main.js"),
        "bmi compiled module missing from frame: {body}"
    );
    assert!(
        body.contains("assets/react.production.min.js")
            && body.contains("assets/react-dom.production.min.js"),
        "bmi react vendor assets missing from frame: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/bmi-calculator/__terrane/frame/assets/app.css",
        None,
    );
    assert_eq!(status, 200, "bmi css: {body}");
    assert!(
        body.contains(".bmi-app"),
        "bmi css missing app styles: {body}"
    );

    let (status, headers, body) = http_raw_with_headers(
        &addr,
        "GET",
        "/apps/bmi-calculator/__terrane/frame/assets/modules/src/main.js",
        None,
        &[],
    );
    assert_eq!(status, 200, "bmi module: {body}");
    // The frame iframe is sandboxed without `allow-same-origin`, so its origin
    // is opaque and the browser fetches `<script type="module">` assets in CORS
    // mode. Without this header the module is blocked and the app renders a
    // blank stage even though every asset serves 200.
    assert!(
        headers
            .lines()
            .any(|l| l.to_ascii_lowercase().starts_with("access-control-allow-origin:")),
        "bmi module missing CORS header for the sandboxed frame: {headers}"
    );
    assert!(
        body.contains("terrane-react-jsx-runtime")
            && body.contains("terrane.invoke")
            && body.contains("createRoot"),
        "bmi module missing compiled React/runtime path: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/bmi-calculator/__terrane/frame/assets/terrane-react-jsx-runtime.js",
        None,
    );
    assert_eq!(status, 200, "bmi jsx runtime wrapper: {body}");
    assert!(
        body.contains("ReactGlobal.Fragment") && body.contains("ReactGlobal.createElement"),
        "bmi jsx runtime wrapper missing React export bridge: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/apps/bmi-calculator/invoke",
        Some(r#"{"verb":"calculate","args":["180","81"]}"#),
    );
    assert_eq!(status, 200, "bmi invoke: {body}");
    assert!(body.contains(r#"\"bmi\":25"#), "bmi output: {body}");
    assert!(
        body.contains(r#"\"category\":\"Overweight\""#),
        "bmi category: {body}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn serves_catalog_ui_and_invoke_over_http() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let built_react = write_built_react_fixture(dir.path());
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo"); // has a UI
        grant_resource(&mut core, "todo", "kv");
        install_from_source(&mut core, "built-react", &built_react); // built React UI
        install(&mut core, "todo-cli-collaborate"); // crdt add/list
        grant_resource(&mut core, "todo-cli-collaborate", "crdt");
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
            && body.contains("built-react")
            && body.contains("\"todo\""),
        "apps: {body}"
    );

    // Shell: wraps the app in host-owned navigation; the browser loads the
    // dynamic app list from `/apps`.
    let (status, body) = http(&addr, "GET", "/apps/todo/", None);
    assert_eq!(status, 200, "shell body: {body}");
    assert!(body.contains("Terrane"), "shell brand missing: {body}");
    assert!(
        body.contains(r#"<a class="brand" href="/""#),
        "brand should link back to the landing page: {body}"
    );
    assert!(
        body.contains("window.terraneAppIcon"),
        "shared app icons missing from shell: {body}"
    );
    // Top bar: breadcrumb (app / editable doc name), user menu with settings,
    // theme switcher, login/logout, and a settings panel beside the iframe.
    assert!(
        body.contains("id=\"crumb-app\"")
            && body.contains("id=\"crumb-doc\"")
            && body.contains("contenteditable"),
        "breadcrumb missing from topbar: {body}"
    );
    assert!(
        body.contains("id=\"user-button\"") && body.contains("id=\"user-dropdown\""),
        "user menu missing from topbar: {body}"
    );
    assert!(
        body.contains("id=\"menu-settings\"") && body.contains("id=\"menu-auth\""),
        "settings / login menu items missing: {body}"
    );
    assert!(
        body.contains("data-theme=\"light\"")
            && body.contains("data-theme=\"dark\"")
            && body.contains("data-theme=\"system\""),
        "theme options missing: {body}"
    );
    assert!(
        body.contains("id=\"settings-panel\"") && body.contains("id=\"settings-close\""),
        "settings panel missing: {body}"
    );
    assert!(
        body.contains("terrane:document") && body.contains("/__terrane/admin/session"),
        "topbar wiring missing from shell script: {body}"
    );
    assert!(
        body.contains("window.__terraneLiveReload = true"),
        "shell should enable catalog polling under live reload: {body}"
    );
    assert!(
        body.contains("id=\"desktop-info-button\"")
            && body.contains("setInfoPanelOpen")
            && body.contains("Terrane desktop app"),
        "desktop editing info missing: {body}"
    );
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
    assert!(
        body.contains(
            "sandbox=\"allow-scripts allow-forms allow-modals allow-popups allow-downloads\""
        ),
        "app frame should be sandboxed away from admin origin: {body}"
    );
    assert!(
        body.contains("terrane:bridge:request"),
        "app shell should expose the iframe bridge: {body}"
    );
    let (status, body) = http_with_headers(
        &addr,
        "OPTIONS",
        "/apps/todo/invoke",
        None,
        &[
            ("Origin", "null"),
            ("Access-Control-Request-Headers", "content-type"),
        ],
    );
    assert_eq!(
        status, 404,
        "app invoke should not expose CORS preflight: {body}"
    );
    let (status, headers, body) = http_raw_with_headers(
        &addr,
        "POST",
        "/apps/todo/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
        &[("Origin", "https://evil.example")],
    );
    assert_eq!(status, 200, "same-origin HTTP invoke still works: {body}");
    assert!(
        !headers
            .to_ascii_lowercase()
            .contains("\r\naccess-control-allow-origin:"),
        "app invoke must not be readable cross-origin: {headers}"
    );
    let (status, _body) = http_without_admin(&addr, "OPTIONS", "/__terrane/admin/grants", None);
    assert_eq!(status, 404, "admin routes should not expose CORS preflight");

    // Private in-memory preview routes: create a generated bundle, serve the
    // iframe HTML/assets, inject the preview invoke shim, and invoke its backend.
    let preview_body = r#"{"files":[{"path":"manifest.json","content":"{\"id\":\"web-demo\",\"name\":\"Web Demo\",\"version\":\"0.1.0\",\"backend\":\"main.js\",\"ui\":\"index.html\",\"resources\":[]}"},{"path":"main.js","content":"var actions={hello:{summary:\"Return a greeting.\",args:[],returns:\"a greeting line.\",run:function(){return \"Hello from web preview\";}}};"},{"path":"index.html","content":"<!doctype html><html><head><title>Web Demo</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><button id=\"hello\">Hello</button></body></html>"},{"path":"style.css","content":"body { color: rgb(1, 2, 3); }"}]}"#;
    let (status, body) = http(&addr, "POST", "/__terrane/previews", Some(preview_body));
    assert_eq!(status, 200, "create preview: {body}");
    assert!(
        body.contains(r#""id":"preview-web-demo-1""#)
            && body.contains(r#""frameUrl":"/__terrane/previews/preview-web-demo-1/frame/""#),
        "create preview body: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/__terrane/previews/preview-web-demo-1/frame/",
        None,
    );
    assert_eq!(status, 200, "preview frame: {body}");
    assert!(
        body.contains("window.terrane"),
        "preview shim missing: {body}"
    );
    assert!(
        body.contains("/__terrane/previews/preview-web-demo-1/invoke"),
        "preview invoke route missing: {body}"
    );
    assert!(body.contains("Web Demo"), "preview HTML missing: {body}");

    let (status, body) = http(
        &addr,
        "GET",
        "/__terrane/previews/preview-web-demo-1/frame/style.css",
        None,
    );
    assert_eq!(status, 200, "preview css: {body}");
    assert!(body.contains("rgb(1, 2, 3)"), "preview css: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/previews/preview-web-demo-1/invoke",
        Some(r#"{"verb":"hello","args":[]}"#),
    );
    assert_eq!(status, 200, "preview invoke: {body}");
    assert!(
        body.contains("Hello from web preview"),
        "preview invoke body: {body}"
    );

    let (status, body) = http(&addr, "GET", "/apps", None);
    assert_eq!(status, 200);
    assert!(
        !body.contains("preview-web-demo-1"),
        "preview app must stay out of catalog: {body}"
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

    // Built React frame: manifest.ui points at dist/index.html, and frame
    // assets resolve relative to that built entry directory.
    let (status, body) = http(&addr, "GET", "/apps/built-react/__terrane/frame/", None);
    assert_eq!(status, 200, "built react frame body: {body}");
    assert!(body.contains("window.terrane"), "shim missing: {body}");
    assert!(
        body.contains("\"built-react\""),
        "app id value missing: {body}"
    );
    assert!(
        body.contains("__terrane/live-version"),
        "live reload hook missing: {body}"
    );
    assert!(
        body.contains("Built React Fixture") && body.contains("./assets/index.js"),
        "built frame missing bundled asset references: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/built-react/__terrane/frame/assets/index.js",
        None,
    );
    assert_eq!(status, 200, "built react asset: {body}");
    assert!(
        body.contains("dataset.builtReact"),
        "built react asset missing: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/built-react/dist/assets/index.css",
        None,
    );
    assert_eq!(status, 200, "direct built asset: {body}");
    assert!(
        body.contains("system-ui"),
        "direct built asset missing: {body}"
    );

    let (status, body) = http(
        &addr,
        "GET",
        "/apps/built-react/__terrane/react/react.js",
        None,
    );
    assert_eq!(status, 404, "removed react runtime route: {body}");

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
    assert!(
        body.contains("\"resources\"") && body.contains("\"prompts\""),
        "mcp initialize capabilities: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":12,"method":"tools/list"}"#),
    );
    assert_eq!(status, 200, "mcp tools/list: {body}");
    assert!(
        body.contains("list_apps")
            && body.contains("app_actions")
            && body.contains("invoke")
            && body.contains("workflows_list")
            && body.contains("workflow_info")
            && body.contains("app_scaffold")
            && body.contains("app_build_start")
            && body.contains("app_build_put_file")
            && body.contains("app_build_validate")
            && body.contains("app_build_commit")
            && body.contains("app_bundle_validate")
            && body.contains("app_register_inline")
            && body.contains("app_register")
            && body.contains("capabilities_list")
            && body.contains("capability_info")
            && body.contains("capability_query")
            && body.contains("capability_command"),
        "mcp tools/list: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(r#"{"jsonrpc":"2.0","id":"resources","method":"resources/list"}"#),
    );
    assert_eq!(status, 200, "mcp resources/list: {body}");
    assert!(
        body.contains("terrane://docs/index") && body.contains("terrane://docs/app-building"),
        "mcp resources/list: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"prompt","method":"prompts/get","params":{"name":"make_js_kv_app","arguments":{"id":"web-notes","name":"Web Notes"}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp prompts/get: {body}");
    assert!(
        body.contains("app_build_start")
            && body.contains("app_register_inline")
            && body.contains("web-notes"),
        "mcp prompts/get: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"query","method":"tools/call","params":{"name":"capability_query","arguments":{"capability":"app","query":"exists","args":["todo-cli-collaborate"]}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp capability_query: {body}");
    assert!(
        body.contains(r#"\"value\":true"#)
            && body.contains(r#""isError":false"#)
            && body.contains(r#""structuredContent""#),
        "mcp capability_query: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"workflow","method":"tools/call","params":{"name":"workflow_info","arguments":{"name":"make_js_kv_app"}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp workflow_info: {body}");
    assert!(
        body.contains("app_build_start")
            && body.contains("app_build_commit")
            && body.contains("app_register_inline")
            && body.contains("app_register")
            && body.contains(r#""structuredContent""#),
        "mcp workflow_info: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"build-start","method":"tools/call","params":{"name":"app_build_start","arguments":{"id":"web-staged","name":"Web Staged","withUi":true}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp app_build_start: {body}");
    assert!(
        body.contains(r#""isError":false"#)
            && body.contains("draftId")
            && body.contains("app_build_validate"),
        "mcp app_build_start: {body}"
    );
    let draft_id = json_string_field(&body, "draftId");

    let validate_body = format!(
        r#"{{"jsonrpc":"2.0","id":"build-validate","method":"tools/call","params":{{"name":"app_build_validate","arguments":{{"draftId":"{draft_id}"}}}}}}"#
    );
    let (status, body) = http(&addr, "POST", "/mcp", Some(&validate_body));
    assert_eq!(status, 200, "mcp app_build_validate: {body}");
    assert!(
        body.contains(r#""isError":false"#)
            && body.contains(r#""valid":true"#)
            && body.contains("validationToken")
            && body.contains("app_build_commit"),
        "mcp app_build_validate: {body}"
    );
    let validation_token = json_string_field(&body, "validationToken");

    let commit_body = format!(
        r#"{{"jsonrpc":"2.0","id":"build-commit","method":"tools/call","params":{{"name":"app_build_commit","arguments":{{"draftId":"{draft_id}","validationToken":"{validation_token}"}}}}}}"#
    );
    let (status, body) = http(&addr, "POST", "/mcp", Some(&commit_body));
    assert_eq!(status, 200, "mcp app_build_commit: {body}");
    assert!(
        body.contains(r#""isError":false"#) && body.contains("web-staged"),
        "mcp app_build_commit: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"build-exists","method":"tools/call","params":{"name":"capability_query","arguments":{"capability":"app","query":"exists","args":["web-staged"]}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp staged app.exists: {body}");
    assert!(
        body.contains(r#"\"value\":true"#) && body.contains(r#""isError":false"#),
        "mcp staged app.exists: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"workflow-multi","method":"tools/call","params":{"name":"workflow_info","arguments":{"name":"make_js_multicap_app_no_filesystem"}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp multicap workflow_info: {body}");
    assert!(
        body.contains("js_multicap_audit")
            && body.contains("replica.peer")
            && body.contains("relational_db"),
        "mcp multicap workflow_info: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"dry","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"app.add","args":["web-dry","Web Dry"],"dryRun":true}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp capability_command dryRun: {body}");
    assert!(
        body.contains(r#"\"dryRun\":true"#) && body.contains(r#""isError":false"#),
        "mcp capability_command dryRun: {body}"
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
fn admin_can_grant_missing_app_resource() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo");
    }

    let (mut child, addr) = spawn_web(home);

    let (status, body) = http(&addr, "GET", "/__terrane/admin", None);
    assert_eq!(status, 200, "admin page: {body}");
    assert!(body.contains("Terrane Admin"), "admin page: {body}");

    let (status, body) = http_without_admin(&addr, "GET", "/__terrane/admin/session", None);
    assert_eq!(
        status, 403,
        "admin control route should require header: {body}"
    );

    let (status, body) = http(&addr, "GET", "/__terrane/admin/session", None);
    assert_eq!(status, 200, "admin session: {body}");
    assert!(
        body.contains(r#""locked":false"#),
        "admin should start unlocked: {body}"
    );

    let (status, body) = http(&addr, "GET", "/__terrane/admin/apps", None);
    assert_eq!(status, 200, "admin apps: {body}");
    assert!(
        body.contains(r#""namespace":"kv""#) && body.contains(r#""granted":false"#),
        "admin apps should show missing kv: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/apps/todo/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
    );
    assert_eq!(status, 403, "invoke should need permission: {body}");
    assert!(
        body.contains("permission_required")
            && body.contains(r#""source":"web""#)
            && body.contains(&format!("http://{addr}/__terrane/admin/requests/")),
        "permission body: {body}"
    );
    let request_id = json_string_field(&body, "requestId");

    let (status, body) = http(&addr, "GET", "/__terrane/admin/requests", None);
    assert_eq!(status, 200, "admin requests: {body}");
    assert!(
        body.contains(&request_id) && body.contains(r#""status":"pending"#),
        "pending request should be listed: {body}"
    );

    let (status, body) = http(&addr, "POST", "/__terrane/admin/local/lock", None);
    assert_eq!(status, 200, "admin lock: {body}");
    assert!(body.contains(r#""locked":true"#), "admin lock: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/admin/grants",
        Some(r#"{"app":"todo","namespace":"kv"}"#),
    );
    assert_eq!(status, 403, "locked admin grant should fail: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/approve"),
        Some(r#"{"reason":"locked"}"#),
    );
    assert_eq!(status, 403, "locked admin approve should fail: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/admin/agents",
        Some(r#"{"id":"codex-local","display_name":"Codex Local"}"#),
    );
    assert_eq!(
        status, 403,
        "locked admin agent register should fail: {body}"
    );

    let (status, body) = http(&addr, "POST", "/__terrane/admin/local/unlock", None);
    assert_eq!(status, 200, "admin unlock: {body}");
    assert!(body.contains(r#""locked":false"#), "admin unlock: {body}");

    let agent = "agent:local-owner:codex-local";
    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/admin/agents",
        Some(
            r#"{"id":"codex-local","display_name":"Codex Local","max_role":"developer","can_install_apps":"true","can_request_permissions":"true","can_grant_permissions":"false"}"#,
        ),
    );
    assert_eq!(status, 200, "admin agent register: {body}");
    assert!(
        body.contains(agent) && body.contains(r#""status":"active"#),
        "admin agent register: {body}"
    );

    let (status, body) = http(&addr, "GET", "/__terrane/admin/agents", None);
    assert_eq!(status, 200, "admin agents: {body}");
    assert!(
        body.contains(agent) && body.contains("Codex Local"),
        "admin agents should list local agent: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/agents/{agent}/delegate"),
        Some(
            r#"{"max_role":"operator","can_install_apps":"false","can_request_permissions":"true","can_grant_permissions":"false"}"#,
        ),
    );
    assert_eq!(status, 200, "admin agent delegate: {body}");
    assert!(
        body.contains(r#""max_role":"operator"#) && body.contains(r#""can_install_apps":false"#),
        "admin agent delegate: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/__terrane/admin/grants",
        Some(&format!(
            r#"{{"app":"todo","namespace":"kv","subject":"{agent}"}}"#
        )),
    );
    assert_eq!(status, 200, "admin grant agent resource: {body}");

    let (status, body) = http(&addr, "GET", "/__terrane/admin/grants", None);
    assert_eq!(status, 200, "admin grants after agent grant: {body}");
    assert!(
        body.contains(agent) && body.contains(r#""namespace":"kv"#),
        "admin grants should include agent grant: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        &format!("/__terrane/admin/requests/{request_id}/approve"),
        Some(r#"{"reason":"ok"}"#),
    );
    assert_eq!(status, 200, "admin approve: {body}");
    assert!(
        body.contains(r#""status":"approved"#),
        "admin approve: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/apps/todo/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
    );
    assert_eq!(status, 200, "invoke after grant: {body}");

    let (status, body) = http(&addr, "POST", "/__terrane/admin/local/lock", None);
    assert_eq!(status, 200, "admin relock: {body}");
    assert!(body.contains(r#""locked":true"#), "admin relock: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/apps/todo/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
    );
    assert_eq!(
        status, 200,
        "lock should not remove existing runtime grant: {body}"
    );

    let (status, body) = http(
        &addr,
        "DELETE",
        &format!("/__terrane/admin/agents/{agent}"),
        None,
    );
    assert_eq!(status, 403, "locked admin agent revoke should fail: {body}");

    let (status, body) = http(
        &addr,
        "DELETE",
        "/__terrane/admin/grants",
        Some(r#"{"app":"todo","namespace":"kv"}"#),
    );
    assert_eq!(status, 403, "locked admin revoke should fail: {body}");

    let (status, body) = http(&addr, "POST", "/__terrane/admin/local/unlock", None);
    assert_eq!(status, 200, "admin unlock before revoke: {body}");

    let (status, body) = http(
        &addr,
        "DELETE",
        &format!("/__terrane/admin/agents/{agent}"),
        None,
    );
    assert_eq!(status, 200, "admin agent revoke: {body}");
    assert!(
        body.contains(agent) && body.contains(r#""status":"revoked"#),
        "admin agent revoke: {body}"
    );

    let (status, body) = http(
        &addr,
        "DELETE",
        "/__terrane/admin/grants",
        Some(r#"{"app":"todo","namespace":"kv"}"#),
    );
    assert_eq!(status, 200, "admin revoke: {body}");

    let (status, body) = http(
        &addr,
        "POST",
        "/apps/todo/invoke",
        Some(r#"{"verb":"list","args":[]}"#),
    );
    assert_eq!(
        status, 403,
        "invoke after revoke should need permission: {body}"
    );

    let (status, body) = http(&addr, "GET", "/__terrane/admin/audit", None);
    assert_eq!(status, 200, "admin audit: {body}");
    assert!(
        body.contains("permission request")
            && body.contains("approved permission request")
            && body.contains("registered agent")
            && body.contains("updated agent delegation")
            && body.contains("revoked agent")
            && body.contains("revoked user:local-owner access"),
        "admin audit should include auth history: {body}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn mcp_http_permission_request_reports_http_source() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo");
    }

    let (mut child, addr) = spawn_web(home);
    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"mcp-missing","method":"tools/call","params":{"name":"app_actions","arguments":{"app":"todo"}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp app_actions missing grant: {body}");
    assert!(
        body.contains("permission_required") && body.contains(r#"\"source\":\"mcp_http\""#),
        "mcp http permission source: {body}"
    );

    let (status, body) = http(
        &addr,
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":"mcp-command-missing","method":"tools/call","params":{"name":"capability_command","arguments":{"name":"kv.set","args":["todo","note","via http"]}}}"#,
        ),
    );
    assert_eq!(status, 200, "mcp capability_command missing grant: {body}");
    assert!(
        body.contains("permission_required")
            && body.contains(r#"\"source\":\"mcp_http\""#)
            && body.contains(r#"\"operation\":\"capability_command:kv.set\""#)
            && body.contains(r#"\"requestStatus\":\"pending\""#),
        "mcp http capability command permission source: {body}"
    );

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
/// (`terrane_api::host_contract`) declares. This is part of the conformance
/// suite consumers (e.g. premium) run against a pinned checkout.
#[test]
fn web_host_serves_every_declared_route() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo"); // has a UI, so the UI route resolves
        grant_resource(&mut core, "todo", "kv");
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

/// The shell's optional Premium (Google) sign-in: the host injects the
/// configured control-plane URL into the shell page; unset stays local-only
/// with the menu item hidden and a `null` injection.
#[test]
fn shell_injects_premium_url_when_configured() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo");
    }

    // Unconfigured: null injection, menu item present but hidden by default.
    let (mut child, addr) = spawn_web(home);
    let (status, body) = http(&addr, "GET", "/apps/todo", None);
    assert_eq!(status, 200, "shell: {body}");
    assert!(
        body.contains("window.__terranePremiumUrl = null;"),
        "unconfigured shell must inject null: {body}"
    );
    assert!(
        body.contains("id=\"menu-premium\""),
        "premium menu item missing: {body}"
    );
    let _ = child.kill();
    let _ = child.wait();

    // Configured via TERRANE_PREMIUM_URL (trailing slash trimmed).
    let (mut child, addr) = spawn_web_full(
        home,
        "127.0.0.1:0",
        None,
        &[],
        &[("TERRANE_PREMIUM_URL", "http://127.0.0.1:8788/".to_string())],
    );
    let (status, body) = http(&addr, "GET", "/apps/todo", None);
    assert_eq!(status, 200, "shell: {body}");
    assert!(
        body.contains("window.__terranePremiumUrl = \"http://127.0.0.1:8788\";"),
        "configured shell must inject the premium url: {body}"
    );
    assert!(
        body.contains("Sign in with Google"),
        "google sign-in affordance missing: {body}"
    );
    assert!(
        body.contains("id=\"settings-premium\""),
        "premium settings row missing: {body}"
    );
    let _ = child.kill();
    let _ = child.wait();

    // The --premium-url flag works too.
    let (mut child, addr) = spawn_web_full(
        home,
        "127.0.0.1:0",
        None,
        &["--premium-url", "http://localhost:9999"],
        &[],
    );
    let (status, body) = http(&addr, "GET", "/apps/todo", None);
    assert_eq!(status, 200, "shell: {body}");
    assert!(
        body.contains("window.__terranePremiumUrl = \"http://localhost:9999\";"),
        "--premium-url must inject the premium url: {body}"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// The catalog allows loopback cross-origin reads (the Premium dashboard
/// lists this host's apps); foreign origins get no CORS grant.
#[test]
fn apps_catalog_grants_cors_to_loopback_origins_only() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    {
        let mut core = Core::open(home.join("log.bin")).unwrap();
        install(&mut core, "todo");
    }
    let (mut child, addr) = spawn_web(home);

    let (status, headers, _body) = http_raw_with_headers(
        &addr,
        "GET",
        "/apps",
        None,
        &[("Origin", "http://127.0.0.1:8788")],
    );
    assert_eq!(status, 200);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("access-control-allow-origin: http://127.0.0.1:8788"),
        "loopback origin must get a CORS grant: {headers}"
    );

    let (status, headers, _body) = http_raw_with_headers(
        &addr,
        "GET",
        "/apps",
        None,
        &[("Origin", "https://evil.example")],
    );
    assert_eq!(status, 200);
    assert!(
        !headers.to_ascii_lowercase().contains("access-control-allow-origin"),
        "foreign origins must get no CORS grant: {headers}"
    );

    let _ = child.kill();
    let _ = child.wait();
}
