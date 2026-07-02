use tiny_http::Response;

use crate::http::{header, Resp};

/// `GET /` — the shared landing page from `terrane-host`, wired for the web
/// host: catalog fetched client-side from `/apps` (polled when live reload is
/// on, so dev apps appear as they land), app cards linking into the
/// `/apps/{id}/` shell, admin console in the footer.
pub fn page(live_reload: bool) -> Resp {
    let body = terrane_host::home_page(&terrane_host::HomePageOptions {
        app_href_template: "/apps/{id}/",
        catalog_url: Some("/apps"),
        catalog_json: None,
        admin_href: Some("/__terrane/admin"),
        catalog_poll_ms: if live_reload { Some(3000) } else { None },
    });
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}
