//! The shared landing ("home") page every host can serve.
//!
//! One template + client script live here; a host renders the page by passing
//! [`HomePageOptions`] describing how *it* exposes the catalog and opens an
//! app. The web host fetches `/apps` client-side and links to `/apps/{id}/`;
//! native webview hosts inline their discovered catalog and link through
//! their app URL scheme (e.g. `terrane-app://{id}/frame/`).

const HOME_HTML: &str = include_str!("home/home.html");
const HOME_JS: &str = include_str!("home/home.js");
const ICONS_JS: &str = include_str!("home/icons.js");

/// The shared app-icon script: defines `window.terraneAppIcon(id)`, mirroring
/// the macOS sidebar's SF Symbol mapping. Included in the landing page and
/// reusable by host shells that list apps.
pub fn app_icons_js() -> &'static str {
    ICONS_JS
}

/// How a host wires the landing page to its own catalog and app links.
pub struct HomePageOptions<'a> {
    /// Per-app link with an `{id}` placeholder, e.g. `/apps/{id}/` or
    /// `terrane-app://{id}/frame/`. The id is URL-encoded before substitution.
    pub app_href_template: &'a str,
    /// URL the page fetches the catalog from (hosts with an HTTP `/apps`).
    pub catalog_url: Option<&'a str>,
    /// Inline catalog JSON (`{"apps":[{"id","name","icon","has_ui"}]}`) for
    /// hosts without an HTTP catalog route. Takes precedence over `catalog_url`.
    pub catalog_json: Option<&'a str>,
    /// Admin console link for the footer; `None` hides the link.
    pub admin_href: Option<&'a str>,
    /// Re-fetch `catalog_url` every N ms so newly added apps appear without a
    /// reload (dev hosts). `None` fetches once.
    pub catalog_poll_ms: Option<u32>,
    /// The negotiated locale for `<html lang>` and chrome localization; empty
    /// (the `Default`) means English.
    pub locale: &'a str,
    /// The `system`-domain message bundle for localizing the page's chrome;
    /// `None` (the `Default`) leaves the English fallback text in place.
    pub messages: Option<&'a std::collections::BTreeMap<String, String>>,
}

impl<'a> Default for HomePageOptions<'a> {
    fn default() -> Self {
        HomePageOptions {
            app_href_template: "/apps/{id}/",
            catalog_url: None,
            catalog_json: None,
            admin_href: None,
            catalog_poll_ms: None,
            locale: "",
            messages: None,
        }
    }
}

/// Render the landing page HTML for a host's [`HomePageOptions`].
pub fn home_page(options: &HomePageOptions) -> String {
    let locale = if options.locale.is_empty() {
        "en"
    } else {
        options.locale
    };
    let dir = terrane_i18n::dir_for(locale);
    // JS first: the config carries host/user-controlled text, so substituting
    // it last keeps a literal `__HOME_JS__` inside it from being re-expanded.
    HOME_HTML
        .replace("__HOME_JS__", &format!("{ICONS_JS}\n{HOME_JS}"))
        .replace("__HOME_CONFIG__", &config_json(options, locale, dir))
        .replace("__HOME_LANG__", &attr_safe(locale))
        .replace("__HOME_DIR__", dir)
}

/// A locale code is validated (ASCII alnum + hyphen), but keep only that safe
/// subset before dropping it into an HTML attribute.
fn attr_safe(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// The page config, embedded in a `<script type="application/json">` block.
/// All values pass through [`json_string`], which escapes `<` so host- and
/// user-controlled strings (app names in the inline catalog) can never close
/// the surrounding script element.
fn config_json(options: &HomePageOptions, locale: &str, dir: &str) -> String {
    let mut config = String::from("{");
    config.push_str("\"appHref\":");
    config.push_str(&json_string(options.app_href_template));
    if let Some(url) = options.catalog_url {
        config.push_str(",\"catalogUrl\":");
        config.push_str(&json_string(url));
    }
    if let Some(json) = options.catalog_json {
        config.push_str(",\"catalog\":");
        config.push_str(&json_string(json));
    }
    if let Some(href) = options.admin_href {
        config.push_str(",\"adminHref\":");
        config.push_str(&json_string(href));
    }
    if let Some(ms) = options.catalog_poll_ms {
        config.push_str(",\"catalogPollMs\":");
        config.push_str(&ms.to_string());
    }
    config.push_str(",\"locale\":");
    config.push_str(&json_string(locale));
    config.push_str(",\"dir\":");
    config.push_str(&json_string(dir));
    if let Some(messages) = options.messages {
        config.push_str(",\"messages\":");
        config.push_str(&json_object(messages));
    }
    config.push('}');
    config
}

/// A JSON object literal with every key and value escaped via [`json_string`],
/// so a bundle string cannot close the surrounding `<script>` block.
fn json_object(map: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in map.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(key));
        out.push(':');
        out.push_str(&json_string(value));
    }
    out.push('}');
    out
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
