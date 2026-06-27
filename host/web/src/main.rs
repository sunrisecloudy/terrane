//! terrane-web — serve a terrane home's apps over HTTP.
//!
//! A thin host over the `terrane_cli`/`terrane-core` spine, like the CLI and MCP
//! hosts. It implements the [`terrane_api`] HTTP contract with `tiny_http`
//! (blocking, single-threaded — one `Core`, one request at a time, which suits
//! the non-`Send` `Core`). It serves app UIs and accepts invokes, injecting a
//! `window.terrane.invoke` shim so an app runs unchanged on the web that runs in
//! the macOS webview.
//!
//! Usage: `terrane-web [--addr 127.0.0.1:8780]`. Loopback binds need no auth;
//! a non-loopback bind requires `TERRANE_WEB_TOKEN` and an
//! `Authorization: Bearer <token>` header on every request.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use nanoserde::{DeJson, SerJson};
use terrane_api::{
    AppSummary, AppsResponse, ApiError, HealthResponse, InvokeRequest, InvokeResponse,
    CONTRACT_VERSION,
};
use terrane_cli::EdgeRunner;
use terrane_core::Core;
use terrane_domain::Request as CoreRequest;
use tiny_http::{Header, Method, Request, Response, Server};

type Resp = Response<Cursor<Vec<u8>>>;

const DEFAULT_ADDR: &str = "127.0.0.1:8780";

fn main() {
    let addr = parse_addr();
    let require_auth = !is_loopback(&addr);
    let token = std::env::var("TERRANE_WEB_TOKEN").ok();
    if require_auth && token.as_deref().map(str::is_empty).unwrap_or(true) {
        eprintln!("terrane-web: a non-loopback bind ({addr}) requires TERRANE_WEB_TOKEN");
        std::process::exit(1);
    }

    let mut core = match open_core() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-web: {e}");
            std::process::exit(1);
        }
    };

    let server = match Server::http(&addr) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("terrane-web: cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "terrane-web: serving {} on http://{} (auth: {})",
        terrane_cli::log_path().display(),
        server.server_addr(),
        if require_auth { "bearer token" } else { "off (loopback)" }
    );

    for mut request in server.incoming_requests() {
        let response = route(&mut core, &mut request, require_auth, token.as_deref());
        let _ = request.respond(response);
    }
}

fn open_core() -> Result<Core<EdgeRunner>, String> {
    let mut core = Core::open_with(terrane_cli::log_path(), EdgeRunner).map_err(|e| e.to_string())?;
    if core.state().replica.peer.is_none() {
        core.dispatch(CoreRequest::new("replica.init", Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    Ok(core)
}

fn route(core: &mut Core<EdgeRunner>, request: &mut Request, require_auth: bool, token: Option<&str>) -> Resp {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    if require_auth && !authorized(request, token) {
        return json_error(401, "unauthorized");
    }

    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match (&method, segs.as_slice()) {
        (Method::Get, ["healthz"]) => json_ok(&HealthResponse {
            status: "ok".into(),
            version: CONTRACT_VERSION.into(),
        }),
        (Method::Get, ["apps"]) => json_ok(&list_apps(core)),
        (Method::Post, ["apps", id, "invoke"]) => invoke(core, id, request),
        (Method::Get, ["apps", id]) => serve_ui(core, id, ""),
        (Method::Get, ["apps", id, rest @ ..]) => serve_ui(core, id, &rest.join("/")),
        _ => json_error(404, "not found"),
    }
}

/// `POST /apps/{id}/invoke` — run a verb on the app's backend, return its output.
fn invoke(core: &mut Core<EdgeRunner>, id: &str, request: &mut Request) -> Resp {
    if !core.state().app.apps.contains_key(id) {
        return json_error(404, &format!("no such app: {id}"));
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: InvokeRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad invoke body: {e}")),
    };

    let mut argv = Vec::with_capacity(parsed.args.len() + 2);
    argv.push(id.to_string());
    argv.push(parsed.verb);
    argv.extend(parsed.args);
    match core.dispatch(CoreRequest::new("host.run", argv)) {
        Ok(_) => json_ok(&InvokeResponse {
            output: core.take_last_output().unwrap_or_default(),
        }),
        Err(e) => json_error(500, &e.to_string()),
    }
}

/// `GET /apps/{id}/…` — serve the app's UI (with the invoke shim injected) or a
/// bundle asset. `rel` is the path under the bundle dir (empty = the UI entry).
fn serve_ui(core: &mut Core<EdgeRunner>, id: &str, rel: &str) -> Resp {
    let Some(source) = core.state().app.apps.get(id).and_then(|a| a.source.clone()) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    let base = Path::new(&source);

    let target = if rel.is_empty() {
        match read_manifest_ui(&source) {
            Some(ui) => base.join(ui),
            None => return json_error(404, "app has no UI"),
        }
    } else {
        base.join(rel)
    };

    // Path-traversal guard: the resolved file must stay within the bundle dir.
    let Some(safe) = safe_within(base, &target) else {
        return json_error(403, "forbidden");
    };
    let Ok(bytes) = std::fs::read(&safe) else {
        return json_error(404, "not found");
    };

    let ctype = content_type(&safe);
    let body = if ctype.starts_with("text/html") {
        inject_shim(&bytes, id)
    } else {
        bytes
    };
    Response::from_data(body).with_header(header("Content-Type", ctype))
}

fn list_apps(core: &mut Core<EdgeRunner>) -> AppsResponse {
    let apps = core
        .state()
        .app
        .apps
        .values()
        .map(|app| AppSummary {
            id: app.id.clone(),
            name: app.name.clone(),
            has_ui: app.source.as_deref().and_then(read_manifest_ui).is_some(),
        })
        .collect();
    AppsResponse { apps }
}

/// The app's declared UI entry file (`manifest.ui`), if any.
fn read_manifest_ui(source: &str) -> Option<String> {
    terrane_core::cap::host::read_manifest(Path::new(source))
        .ok()
        .map(|m| m.ui)
        .filter(|ui| !ui.is_empty())
}

/// Canonicalize `target` and confirm it stays within `base` — rejects `..`,
/// absolute escapes, and symlink escapes. `None` if outside or missing.
fn safe_within(base: &Path, target: &Path) -> Option<PathBuf> {
    let base = std::fs::canonicalize(base).ok()?;
    let target = std::fs::canonicalize(target).ok()?;
    target.starts_with(&base).then_some(target)
}

/// Inject the `window.terrane.invoke` shim at the top of an HTML document so the
/// page can call its own backend over `/apps/{id}/invoke` — the web twin of the
/// macOS webview bridge.
fn inject_shim(html: &[u8], app_id: &str) -> Vec<u8> {
    let shim = format!(
        "<script>\n\
         window.APP_ID={app};\n\
         window.terrane={{invoke:function(verb){{\
         var args=Array.prototype.slice.call(arguments,1).map(String);\
         return fetch(\"/apps/\"+window.APP_ID+\"/invoke\",{{method:\"POST\",\
         headers:{{\"content-type\":\"application/json\"}},\
         body:JSON.stringify({{verb:verb,args:args}})}})\
         .then(function(r){{return r.json();}})\
         .then(function(j){{if(j.error)throw new Error(j.error);return j.output;}});}}}};\n\
         </script>\n",
        app = js_string(app_id)
    );
    let text = String::from_utf8_lossy(html);
    // Insert right after <head> if present, else at the very top.
    let injected = match text.find("<head>") {
        Some(i) => {
            let cut = i + "<head>".len();
            format!("{}{}{}", &text[..cut], shim, &text[cut..])
        }
        None => format!("{shim}{text}"),
    };
    injected.into_bytes()
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

// --- HTTP helpers -------------------------------------------------------------

fn authorized(request: &Request, token: Option<&str>) -> bool {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return false;
    };
    let expected = format!("Bearer {token}");
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == expected)
}

fn json_ok<T: SerJson>(value: &T) -> Resp {
    Response::from_data(value.serialize_json().into_bytes())
        .with_header(header("Content-Type", "application/json"))
}

fn json_error(code: u16, message: &str) -> Resp {
    let body = ApiError { error: message.to_string() }.serialize_json();
    Response::from_data(body.into_bytes())
        .with_status_code(code)
        .with_header(header("Content-Type", "application/json"))
}

fn header(field: &str, value: &str) -> Header {
    // Inputs are all static/known-good ASCII, so this never fails in practice.
    Header::from_bytes(field.as_bytes(), value.as_bytes())
        .unwrap_or_else(|_| Header::from_bytes(&b"X-Terrane"[..], &b"err"[..]).unwrap())
}

/// Minimal JS/JSON string literal for the app id (a slug, but escape defensively).
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '\n' | '\r' => {}
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_addr() -> String {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--addr")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

fn is_loopback(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "::1" | "localhost") || host.starts_with("127.")
}
