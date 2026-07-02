use tiny_http::Response;

use crate::http::{header, Resp};

/// `GET /` — the shared landing page from `terrane-host`, wired for the web
/// host: catalog fetched client-side from `/apps`, app cards linking into the
/// `/apps/{id}/` shell, admin console in the footer.
pub fn page() -> Resp {
    let body = terrane_host::home_page(&terrane_host::HomePageOptions {
        app_href_template: "/apps/{id}/",
        catalog_url: Some("/apps"),
        catalog_json: None,
        admin_href: Some("/__terrane/admin"),
    });
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}
