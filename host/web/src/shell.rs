use std::collections::BTreeMap;

use tiny_http::Response;

use crate::http::{header, json_error, Resp};

const SHELL_HTML: &str = include_str!("templates/shell.html");
const APP_SHELL_JS: &str = include_str!("js/app_shell.js");
const ADMIN_JS: &str = include_str!("js/admin.js");

/// The negotiated locale and the message bundles the shell injects into the
/// page: `system` chrome strings for the shell's own UI, and the app frame's
/// merged bundle (empty in admin mode). Built by the router from the request
/// (Accept-Language / the `terrane_lang` cookie) and the public KV catalog.
pub struct ShellI18n<'a> {
    pub locale: &'a str,
    pub dir: &'a str,
    pub system_messages: &'a BTreeMap<String, String>,
    pub app_messages: &'a BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct AppFramePolicy {
    pub frame_origin: Option<String>,
    pub browser_permissions: Vec<String>,
}

/// `GET /apps/{id}` — the host shell around the app iframe. `exists` is the
/// router's merged catalog + dev-apps check; `live_reload` turns on catalog
/// polling in the sidebar so newly dropped dev apps appear without a refresh.
/// `premium_url` (optional) points the top bar's Google sign-in at a Terrane
/// Premium control plane; unset keeps the shell local-only. `i18n` carries the
/// negotiated locale + message bundles for the chrome and the app frame.
pub fn response(
    exists: bool,
    id: &str,
    live_reload: bool,
    premium_url: Option<&str>,
    frame_policy: &AppFramePolicy,
    i18n: &ShellI18n<'_>,
) -> Resp {
    if !exists {
        return json_error(404, &format!("no such app: {id}"));
    }
    html_response(live_reload, premium_url, "\"app\"", frame_policy, i18n)
}

/// `GET /__terrane/admin` — the same host shell, with the admin console mounted
/// in the main stage instead of navigating away from the sidebar/top bar.
pub fn admin_response(live_reload: bool, premium_url: Option<&str>, i18n: &ShellI18n<'_>) -> Resp {
    html_response(
        live_reload,
        premium_url,
        "\"admin\"",
        &AppFramePolicy::default(),
        i18n,
    )
}

fn html_response(
    live_reload: bool,
    premium_url: Option<&str>,
    shell_mode: &str,
    frame_policy: &AppFramePolicy,
    i18n: &ShellI18n<'_>,
) -> Resp {
    let frame_origin = frame_policy
        .frame_origin
        .as_deref()
        .map(json_string_literal)
        .unwrap_or_else(|| "null".to_string());
    let body = SHELL_HTML
        .replace("__APP_ICONS_JS__", terrane_host::home::app_icons_js())
        .replace("__APP_SHELL_JS__", APP_SHELL_JS)
        .replace("__ADMIN_JS__", ADMIN_JS)
        .replace("__APP_FRAME_ORIGIN__", &frame_origin)
        .replace("__APP_FRAME_SANDBOX__", &app_frame_sandbox(frame_policy))
        .replace("__APP_FRAME_ALLOW__", &app_frame_allow(frame_policy))
        .replace(
            "__LIVE_RELOAD__",
            if live_reload { "true" } else { "false" },
        )
        .replace("__SHELL_MODE__", shell_mode)
        .replace("__PREMIUM_URL__", &premium_url_js(premium_url))
        .replace("__TERRANE_LANG__", &attr_safe(i18n.locale))
        .replace(
            "__TERRANE_DIR__",
            if i18n.dir == "rtl" { "rtl" } else { "ltr" },
        )
        .replace("__TERRANE_LOCALE_JSON__", &json_string_literal(i18n.locale))
        .replace(
            "__TERRANE_MESSAGES_JSON__",
            &json_object_literal(i18n.system_messages),
        )
        .replace(
            "__TERRANE_APP_MESSAGES_JSON__",
            &json_object_literal(i18n.app_messages),
        );
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}

fn app_frame_sandbox(frame_policy: &AppFramePolicy) -> String {
    let base = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";
    if frame_policy.frame_origin.is_some() && !frame_policy.browser_permissions.is_empty() {
        format!("{base} allow-same-origin")
    } else {
        base.to_string()
    }
}

fn app_frame_allow(frame_policy: &AppFramePolicy) -> String {
    let mut permissions = frame_policy
        .browser_permissions
        .iter()
        .filter_map(|permission| match permission.as_str() {
            "camera" | "microphone" => Some(permission.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    permissions.sort_unstable();
    permissions.dedup();
    if permissions.is_empty() {
        return String::new();
    }
    format!("allow=\"{}\"", permissions.join("; "))
}

/// The premium URL as a JS literal — `null` when unconfigured, otherwise a
/// JSON string with quotes/backslashes/angles escaped so the template stays
/// inert markup.
fn premium_url_js(premium_url: Option<&str>) -> String {
    match premium_url {
        None => "null".to_string(),
        Some(url) => json_string_literal(url),
    }
}

/// A JS string literal with the characters that could break out of a `<script>`
/// (or an attribute) escaped. Trusted host-owned content, but escaped anyway —
/// the same discipline as the former `premium_url_js`.
fn json_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON object literal `{ "key": "value", … }` with every key and value
/// escaped via [`json_string_literal`], so a bundle string cannot break the
/// injected `<script>`.
fn json_object_literal(map: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in map.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string_literal(key));
        out.push(':');
        out.push_str(&json_string_literal(value));
    }
    out.push('}');
    out
}

/// A locale code is a validated supported code (ASCII alnum + hyphen), but keep
/// only that safe subset before dropping it into an HTML attribute.
fn attr_safe(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}
