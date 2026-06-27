use std::path::Path;

use nanoserde::DeJson;
use terrane_api::{HealthResponse, InvokeRequest, InvokeResponse, CONTRACT_VERSION};
use terrane_host::{PreviewFile, PreviewStore};
use tiny_http::{Method, Request, Response};

use crate::http::{authorized, header, json_error, json_ok, Resp};
use crate::shim::{inject_app_shim, inject_preview_shim};
use crate::static_files::{content_type, safe_within};

#[derive(DeJson)]
struct CreatePreviewRequest {
    files: Vec<PreviewFile>,
}

pub fn route(
    core: &mut terrane_host::HostCore,
    previews: &mut PreviewStore,
    request: &mut Request,
    require_auth: bool,
    token: Option<&str>,
    live_reload: bool,
) -> Resp {
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
        (Method::Post, ["__terrane", "previews"]) => create_preview(core, previews, request),
        (Method::Get, ["__terrane", "previews", id, "frame"]) => serve_preview(previews, id, ""),
        (Method::Get, ["__terrane", "previews", id, "frame", rest @ ..]) => {
            serve_preview(previews, id, &rest.join("/"))
        }
        (Method::Post, ["__terrane", "previews", id, "invoke"]) => {
            invoke_preview(previews, id, request)
        }
        (Method::Get | Method::Post, ["__terrane", "previews", ..]) => json_error(404, "not found"),
        (Method::Get, ["apps"]) => json_ok(&terrane_host::list_apps(core)),
        (Method::Post, ["mcp"]) => mcp(core, request),
        (Method::Get, ["mcp"]) => json_error(405, "method not allowed"),
        (Method::Get, ["apps", id, "__terrane", "live-version"]) if live_reload => {
            crate::live_reload::response(core, id)
        }
        (Method::Get, ["apps", id, "__terrane", "frame"]) => serve_ui(core, id, "", live_reload),
        (Method::Get, ["apps", id, "__terrane", "frame", rest @ ..]) => {
            serve_ui(core, id, &rest.join("/"), live_reload)
        }
        (Method::Get, ["apps", _id, "__terrane", ..]) => json_error(404, "not found"),
        (Method::Post, ["apps", id, "invoke"]) => invoke(core, id, request),
        (Method::Get, ["apps", id]) => crate::shell::response(core, id),
        (Method::Get, ["apps", id, rest @ ..]) => {
            serve_bundle_asset(core, id, &rest.join("/"), live_reload)
        }
        _ => json_error(404, "not found"),
    }
}

/// `POST /__terrane/previews` - create an in-memory app preview from files.
fn create_preview(
    core: &terrane_host::HostCore,
    previews: &mut PreviewStore,
    request: &mut Request,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: CreatePreviewRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad preview body: {e}")),
    };

    match previews.create_preview(parsed.files, core.state()) {
        Ok(mut response) => {
            response.frame_url = format!("/__terrane/previews/{}/frame/", response.id);
            json_ok(&response)
        }
        Err(e) => json_error(400, &e),
    }
}

/// `POST /__terrane/previews/{id}/invoke` - run the generated preview backend.
fn invoke_preview(previews: &mut PreviewStore, id: &str, request: &mut Request) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: InvokeRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad invoke body: {e}")),
    };

    match previews.invoke_backend(id, &parsed.verb, &parsed.args) {
        Ok(output) => json_ok(&InvokeResponse { output }),
        Err(e) if e.starts_with("no such preview:") => json_error(404, &e),
        Err(e) => json_error(500, &e),
    }
}

/// `GET /__terrane/previews/{id}/frame/...` - serve generated UI/assets.
fn serve_preview(previews: &PreviewStore, id: &str, rel: &str) -> Resp {
    let asset = match previews.read_asset(id, rel) {
        Ok(asset) => asset,
        Err(e) if e.starts_with("no such preview:") => return json_error(404, &e),
        Err(e) if e.contains("absolute paths") || e.contains("parent-dir") => {
            return json_error(403, &e)
        }
        Err(e) => return json_error(404, &e),
    };
    let content_type = asset.content_type;
    let body = if content_type.starts_with("text/html") {
        inject_preview_shim(asset.content.as_bytes(), id)
    } else {
        asset.content.into_bytes()
    };
    Response::from_data(body).with_header(header("Content-Type", &content_type))
}

/// `POST /mcp` — MCP JSON-RPC over HTTP, backed by the shared host MCP module.
fn mcp(core: &mut terrane_host::HostCore, request: &mut Request) -> Resp {
    if !origin_allowed(request) {
        return json_error(403, "forbidden origin");
    }

    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }

    match terrane_host::mcp::handle_json_rpc(core, &body) {
        Some(response) => Response::from_data(response.into_bytes())
            .with_header(header("Content-Type", "application/json")),
        None => Response::from_string("").with_status_code(202),
    }
}

/// `POST /apps/{id}/invoke` — run a verb on the app's backend, return its output.
fn invoke(core: &mut terrane_host::HostCore, id: &str, request: &mut Request) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: InvokeRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad invoke body: {e}")),
    };

    match terrane_host::invoke_app(core, id, &parsed.verb, &parsed.args) {
        Ok(output) => json_ok(&InvokeResponse { output }),
        Err(e) if e.starts_with("no such app:") => json_error(404, &e),
        Err(e) => json_error(500, &e),
    }
}

/// `GET /apps/{id}/…` — serve the app's UI (with the invoke shim injected) or a
/// bundle asset. `rel` is the path under the bundle dir (empty = the UI entry).
fn serve_ui(core: &mut terrane_host::HostCore, id: &str, rel: &str, live_reload: bool) -> Resp {
    let Some(source) = core.state().app.apps.get(id).and_then(|a| a.source.clone()) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    let base = Path::new(&source);

    let Some(ui) = terrane_host::read_manifest_ui(&source) else {
        return json_error(404, "app has no UI");
    };
    let entry = base.join(ui);
    let target = if rel.is_empty() {
        entry
    } else {
        entry.parent().unwrap_or(base).join(rel)
    };

    serve_file(id, base, &target, live_reload)
}

/// `GET /apps/{id}/{rel}` — serve a direct bundle asset path rooted at the app
/// source directory. This keeps `/apps/{id}/dist/...` useful for built bundles
/// while the iframe route resolves relative to `manifest.ui`'s directory.
fn serve_bundle_asset(
    core: &mut terrane_host::HostCore,
    id: &str,
    rel: &str,
    live_reload: bool,
) -> Resp {
    let Some(source) = core.state().app.apps.get(id).and_then(|a| a.source.clone()) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    let base = Path::new(&source);
    let target = base.join(rel);

    serve_file(id, base, &target, live_reload)
}

fn serve_file(id: &str, base: &Path, target: &Path, live_reload: bool) -> Resp {
    let Some(safe) = safe_within(base, target) else {
        return json_error(403, "forbidden");
    };
    let Ok(bytes) = std::fs::read(&safe) else {
        return json_error(404, "not found");
    };

    let ctype = content_type(&safe);
    let body = if ctype.starts_with("text/html") {
        inject_app_shim(&bytes, id, live_reload)
    } else {
        bytes
    };
    Response::from_data(body).with_header(header("Content-Type", ctype))
}

fn origin_allowed(request: &Request) -> bool {
    let Some(origin) = header_value(request, "Origin") else {
        return true;
    };
    let Some(origin_host) = origin_host(origin) else {
        return false;
    };
    if is_loopback_host(origin_host) {
        return true;
    }
    header_value(request, "Host")
        .and_then(host_without_port)
        .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
}

fn header_value<'a>(request: &'a Request, field: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.to_string().eq_ignore_ascii_case(field))
        .map(|h| h.value.as_str())
}

fn origin_host(origin: &str) -> Option<&str> {
    let after_scheme = origin.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?;
    host_without_port(authority)
}

fn host_without_port(authority: &str) -> Option<&str> {
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    authority
        .split_once(':')
        .map(|(host, _)| host)
        .or(Some(authority))
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "::1" | "localhost") || host.starts_with("127.")
}
