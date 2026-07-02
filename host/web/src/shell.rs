use tiny_http::Response;

use crate::http::{header, json_error, Resp};

const SHELL_HTML: &str = include_str!("templates/shell.html");
const APP_SHELL_JS: &str = include_str!("js/app_shell.js");

/// `GET /apps/{id}` — the host shell around the app iframe. `exists` is the
/// router's merged catalog + dev-apps check; `live_reload` turns on catalog
/// polling in the sidebar so newly dropped dev apps appear without a refresh.
pub fn response(exists: bool, id: &str, live_reload: bool) -> Resp {
    if !exists {
        return json_error(404, &format!("no such app: {id}"));
    }

    let body = SHELL_HTML
        .replace("__APP_ICONS_JS__", terrane_host::home::app_icons_js())
        .replace("__APP_SHELL_JS__", APP_SHELL_JS)
        .replace("__LIVE_RELOAD__", if live_reload { "true" } else { "false" });
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}
