use tiny_http::Response;

use crate::http::{header, Resp};

const HOME_HTML: &str = include_str!("templates/home.html");
const HOME_JS: &str = include_str!("js/home.js");

/// `GET /` — the landing page: brand, the installed-app catalog (loaded
/// client-side from `/apps`), and a link to the admin console.
pub fn page() -> Resp {
    let body = HOME_HTML.replace("__HOME_JS__", HOME_JS);
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}
