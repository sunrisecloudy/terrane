use std::path::Path;

use nanoserde::DeJson;
use terrane_api::{HealthResponse, InvokeRequest, InvokeResponse, CONTRACT_VERSION};
use tiny_http::{Method, Request, Response};

use crate::http::{authorized, header, json_error, json_ok, Resp};
use crate::shim::inject_shim;
use crate::static_files::{content_type, safe_within};

pub fn route(
    core: &mut terrane_host::HostCore,
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
        (Method::Get, ["apps"]) => json_ok(&terrane_host::list_apps(core)),
        (Method::Get, ["apps", id, "__terrane", "live-version"]) if live_reload => {
            crate::live_reload::response(core, id)
        }
        (Method::Get, ["apps", id, "__terrane", "frame"]) => serve_ui(core, id, "", live_reload),
        (Method::Get, ["apps", id, "__terrane", "frame", rest @ ..]) => {
            serve_ui(core, id, &rest.join("/"), live_reload)
        }
        (Method::Post, ["apps", id, "invoke"]) => invoke(core, id, request),
        (Method::Get, ["apps", id]) => crate::shell::response(core, id),
        (Method::Get, ["apps", id, rest @ ..]) => serve_ui(core, id, &rest.join("/"), live_reload),
        _ => json_error(404, "not found"),
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

    let target = if rel.is_empty() {
        match terrane_host::read_manifest_ui(&source) {
            Some(ui) => base.join(ui),
            None => return json_error(404, "app has no UI"),
        }
    } else {
        base.join(rel)
    };

    let Some(safe) = safe_within(base, &target) else {
        return json_error(403, "forbidden");
    };
    let Ok(bytes) = std::fs::read(&safe) else {
        return json_error(404, "not found");
    };

    let ctype = content_type(&safe);
    let body = if ctype.starts_with("text/html") {
        inject_shim(&bytes, id, live_reload)
    } else {
        bytes
    };
    Response::from_data(body).with_header(header("Content-Type", ctype))
}
