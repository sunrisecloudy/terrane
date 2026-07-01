use std::io::Cursor;

use nanoserde::SerJson;
use terrane_api::ApiError;
use tiny_http::{Header, Request, Response};

pub type Resp = Response<Cursor<Vec<u8>>>;
pub const ADMIN_HEADER: &str = "X-Terrane-Admin";
pub const ADMIN_HEADER_VALUE: &str = "local-admin";

pub fn authorized(request: &Request, token: Option<&str>) -> bool {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return false;
    };
    let expected = format!("Bearer {token}");
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == expected)
}

pub fn admin_authorized(request: &Request) -> bool {
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv(ADMIN_HEADER) && h.value.as_str() == ADMIN_HEADER_VALUE)
}

pub fn json_ok<T: SerJson>(value: &T) -> Resp {
    Response::from_data(value.serialize_json().into_bytes())
        .with_header(header("Content-Type", "application/json"))
}

pub fn json_error(code: u16, message: &str) -> Resp {
    let body = ApiError {
        error: message.to_string(),
    }
    .serialize_json();
    Response::from_data(body.into_bytes())
        .with_status_code(code)
        .with_header(header("Content-Type", "application/json"))
}

pub fn header(field: &str, value: &str) -> Header {
    // Inputs are all static/known-good ASCII, so this never fails in practice.
    Header::from_bytes(field.as_bytes(), value.as_bytes())
        .unwrap_or_else(|_| Header::from_bytes(&b"X-Terrane"[..], &b"err"[..]).unwrap())
}
