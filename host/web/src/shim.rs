const TERRANE_SHIM_JS: &str = include_str!("js/terrane_shim.js");
const LIVE_RELOAD_JS: &str = include_str!("js/live_reload.js");

/// Inject the `window.terrane.invoke` shim at the top of an HTML document so the
/// page can call its own backend over `/apps/{id}/invoke` — the web twin of the
/// macOS webview bridge.
pub fn inject_app_shim(html: &[u8], app_id: &str, live_reload: bool) -> Vec<u8> {
    inject(
        html,
        app_id,
        &format!("/apps/{app_id}/invoke"),
        Some("/__terrane/previews"),
        live_reload,
    )
}

pub fn inject_preview_shim(html: &[u8], preview_id: &str) -> Vec<u8> {
    inject(
        html,
        preview_id,
        &format!("/__terrane/previews/{preview_id}/invoke"),
        None,
        false,
    )
}

fn inject(
    html: &[u8],
    app_id: &str,
    invoke_url: &str,
    preview_url: Option<&str>,
    live_reload: bool,
) -> Vec<u8> {
    let js = TERRANE_SHIM_JS
        .replace("__APP_ID_JSON__", &js_string(app_id))
        .replace("__INVOKE_URL_JSON__", &js_string(invoke_url))
        .replace(
            "__PREVIEW_URL_JSON__",
            &preview_url
                .map(js_string)
                .unwrap_or_else(|| "null".to_string()),
        )
        .replace(
            "__LIVE_RELOAD_SCRIPT__",
            if live_reload { LIVE_RELOAD_JS } else { "" },
        );
    let shim = format!("<script>\n{js}</script>\n");
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
