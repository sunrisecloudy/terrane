use tiny_http::Response;

use crate::http::{header, json_error, Resp};
use crate::shim::inject_shim;

const REACT_SHELL_HTML: &str = include_str!("templates/react_shell.html");
const REACT_JS: &str = include_str!("js/react_runtime.js");
const REACT_DOM_JS: &str = include_str!("js/react_dom_runtime.js");

pub fn response(app_id: &str, entry: &str, live_reload: bool) -> Resp {
    let entry = entry.trim_start_matches('/');
    if entry.is_empty() || entry.split('/').any(|part| part == ".." || part.is_empty()) {
        return json_error(400, "bad react entry");
    }

    let html = REACT_SHELL_HTML
        .replace("__APP_ID_JSON__", &js_string(app_id))
        .replace("__APP_ID__", &html_attr(app_id))
        .replace("__APP_ENTRY__", &html_attr(entry));
    let body = inject_shim(html.as_bytes(), app_id, live_reload);
    Response::from_data(body).with_header(header("Content-Type", "text/html; charset=utf-8"))
}

pub fn runtime_response(name: &str) -> Resp {
    let body = match name {
        "react.js" => REACT_JS,
        "react-dom.js" => REACT_DOM_JS,
        _ => return json_error(404, "react runtime not found"),
    };
    Response::from_data(body.as_bytes().to_vec())
        .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
}

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

fn html_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}
