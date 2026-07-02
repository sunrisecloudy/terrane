use tiny_http::Response;

use crate::http::{header, json_error, Resp};

const SHELL_HTML: &str = include_str!("templates/shell.html");
const APP_SHELL_JS: &str = include_str!("js/app_shell.js");

pub fn response(core: &terrane_host::HostCore, current_id: &str) -> Resp {
    let apps = terrane_host::list_apps(core);
    if !apps.apps.iter().any(|app| app.id == current_id) {
        return json_error(404, &format!("no such app: {current_id}"));
    }

    let body = SHELL_HTML
        .replace("__APP_ICONS_JS__", terrane_host::home::app_icons_js())
        .replace("__APP_SHELL_JS__", APP_SHELL_JS);
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}
