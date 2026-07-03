use tiny_http::Response;

use crate::http::{header, json_error, Resp};

const SHELL_HTML: &str = include_str!("templates/shell.html");
const APP_SHELL_JS: &str = include_str!("js/app_shell.js");
const ADMIN_JS: &str = include_str!("js/admin.js");

/// `GET /apps/{id}` — the host shell around the app iframe. `exists` is the
/// router's merged catalog + dev-apps check; `live_reload` turns on catalog
/// polling in the sidebar so newly dropped dev apps appear without a refresh.
/// `premium_url` (optional) points the top bar's Google sign-in at a Terrane
/// Premium control plane; unset keeps the shell local-only.
pub fn response(exists: bool, id: &str, live_reload: bool, premium_url: Option<&str>) -> Resp {
    if !exists {
        return json_error(404, &format!("no such app: {id}"));
    }

    let body = SHELL_HTML
        .replace("__APP_ICONS_JS__", terrane_host::home::app_icons_js())
        .replace("__APP_SHELL_JS__", APP_SHELL_JS)
        .replace("__ADMIN_JS__", ADMIN_JS)
        .replace(
            "__LIVE_RELOAD__",
            if live_reload { "true" } else { "false" },
        )
        .replace("__SHELL_MODE__", "\"app\"")
        .replace("__PREMIUM_URL__", &premium_url_js(premium_url));
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}

/// `GET /__terrane/admin` — the same host shell, with the admin console mounted
/// in the main stage instead of navigating away from the sidebar/top bar.
pub fn admin_response(live_reload: bool, premium_url: Option<&str>) -> Resp {
    let body = SHELL_HTML
        .replace("__APP_ICONS_JS__", terrane_host::home::app_icons_js())
        .replace("__APP_SHELL_JS__", APP_SHELL_JS)
        .replace("__ADMIN_JS__", ADMIN_JS)
        .replace(
            "__LIVE_RELOAD__",
            if live_reload { "true" } else { "false" },
        )
        .replace("__SHELL_MODE__", "\"admin\"")
        .replace("__PREMIUM_URL__", &premium_url_js(premium_url));
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}

/// The premium URL as a JS literal — `null` when unconfigured, otherwise a
/// JSON string with quotes/backslashes/angles escaped so the template stays
/// inert markup.
fn premium_url_js(premium_url: Option<&str>) -> String {
    match premium_url {
        None => "null".to_string(),
        Some(url) => format!(
            "\"{}\"",
            url.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('<', "\\u003c")
        ),
    }
}
