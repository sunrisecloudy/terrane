//! The shared landing ("home") page every host can serve.
//!
//! One template + client script live here; a host renders the page by passing
//! [`HomePageOptions`] describing how *it* exposes the catalog and opens an
//! app. The web host fetches `/apps` client-side and links to `/apps/{id}/`;
//! native webview hosts inline their discovered catalog and link through
//! their app URL scheme (e.g. `terrane-app://{id}/frame/`).

const HOME_HTML: &str = include_str!("home/home.html");
const HOME_JS: &str = include_str!("home/home.js");

/// How a host wires the landing page to its own catalog and app links.
pub struct HomePageOptions<'a> {
    /// Per-app link with an `{id}` placeholder, e.g. `/apps/{id}/` or
    /// `terrane-app://{id}/frame/`. The id is URL-encoded before substitution.
    pub app_href_template: &'a str,
    /// URL the page fetches the catalog from (hosts with an HTTP `/apps`).
    pub catalog_url: Option<&'a str>,
    /// Inline catalog JSON (`{"apps":[{"id","name","has_ui"}]}`) for hosts
    /// without an HTTP catalog route. Takes precedence over `catalog_url`.
    pub catalog_json: Option<&'a str>,
    /// Admin console link for the footer; `None` hides the link.
    pub admin_href: Option<&'a str>,
}

/// Render the landing page HTML for a host's [`HomePageOptions`].
pub fn home_page(options: &HomePageOptions) -> String {
    // JS first: the config carries host/user-controlled text, so substituting
    // it last keeps a literal `__HOME_JS__` inside it from being re-expanded.
    HOME_HTML
        .replace("__HOME_JS__", HOME_JS)
        .replace("__HOME_CONFIG__", &config_json(options))
}

/// The page config, embedded in a `<script type="application/json">` block.
/// All values pass through [`json_string`], which escapes `<` so host- and
/// user-controlled strings (app names in the inline catalog) can never close
/// the surrounding script element.
fn config_json(options: &HomePageOptions) -> String {
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
    config.push('}');
    config
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
