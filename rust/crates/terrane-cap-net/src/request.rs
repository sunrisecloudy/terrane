use std::collections::BTreeSet;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{Error, Result};

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 120_000;
pub const INLINE_AUTO_LIMIT: usize = 256 * 1024;
pub const INLINE_FORCED_LIMIT: usize = 8 * 1024 * 1024;
pub const BODY_HARD_LIMIT: usize = 32 * 1024 * 1024;
pub const REDACTED: &str = "«redacted»";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HeaderValue>,
    pub body: Option<RequestBody>,
    pub timeout_ms: u64,
    pub redirect: RedirectPolicy,
    pub response_body: ResponseBodyMode,
    pub canonical_json: String,
    pub redacted_json: String,
    pub request_key: String,
    pub has_unresolved_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValue {
    pub name: String,
    pub value: RequestValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestValue {
    Plain(String),
    Secret(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestBody {
    Text(String),
    Base64(Vec<u8>),
    Secret(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    Follow,
    Manual,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseBodyMode {
    Auto,
    Inline,
    Blob,
}

impl RedirectPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            RedirectPolicy::Follow => "follow",
            RedirectPolicy::Manual => "manual",
            RedirectPolicy::Deny => "deny",
        }
    }
}

impl ResponseBodyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ResponseBodyMode::Auto => "auto",
            ResponseBodyMode::Inline => "inline",
            ResponseBodyMode::Blob => "blob",
        }
    }
}

pub fn prepare_request(raw: &str) -> Result<PreparedRequest> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("net request must be JSON object: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("net request must be a JSON object".into()))?;

    let method = match obj.get("method") {
        Some(value) => string_field(value, "method")?.to_ascii_uppercase(),
        None => "GET".to_string(),
    };
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return Err(Error::InvalidInput(format!(
            "unsupported HTTP method: {method}"
        )));
    }

    let url = obj
        .get("url")
        .ok_or_else(|| Error::InvalidInput("net request missing url".into()))
        .and_then(|value| string_field(value, "url"))?;
    if url.trim().is_empty() {
        return Err(Error::InvalidInput("url must not be empty".into()));
    }

    let sensitive = sensitive_headers(obj.get("sensitiveHeaders"))?;
    let headers = parse_headers(obj.get("headers"))?;
    let body = parse_body(obj.get("body"))?;
    let timeout_ms = parse_timeout(obj.get("timeoutMs"))?;
    let redirect = parse_redirect(obj.get("redirect"))?;
    let response_body = parse_response_body(obj.get("responseBody"))?;

    let request = RequestJsonInput {
        method: &method,
        url: &url,
        headers: &headers,
        body: &body,
        timeout_ms,
        redirect,
        response_body,
        sensitive: &sensitive,
    };
    let canonical = request_json(
        &request,
        false,
    );
    let redacted = request_json(
        &request,
        true,
    );
    let canonical_json = serde_json::to_string(&canonical)
        .map_err(|e| Error::InvalidInput(format!("canonicalize net request: {e}")))?;
    let redacted_json = serde_json::to_string(&redacted)
        .map_err(|e| Error::InvalidInput(format!("redact net request: {e}")))?;
    let request_key = sha256_hex(canonical_json.as_bytes());
    let has_unresolved_secret = headers
        .iter()
        .any(|header| matches!(header.value, RequestValue::Secret(_)))
        || matches!(body, Some(RequestBody::Secret(_)));

    Ok(PreparedRequest {
        method,
        url,
        headers,
        body,
        timeout_ms,
        redirect,
        response_body,
        canonical_json,
        redacted_json,
        request_key,
        has_unresolved_secret,
    })
}

fn string_field(value: &Value, name: &str) -> Result<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
}

fn sensitive_headers(value: Option<&Value>) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let Some(value) = value else {
        return Ok(out);
    };
    let array = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("sensitiveHeaders must be an array".into()))?;
    for item in array {
        let name = string_field(item, "sensitiveHeaders item")?.to_ascii_lowercase();
        if name.trim().is_empty() {
            return Err(Error::InvalidInput(
                "sensitiveHeaders names must not be empty".into(),
            ));
        }
        out.insert(name);
    }
    Ok(out)
}

fn parse_headers(value: Option<&Value>) -> Result<Vec<HeaderValue>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("headers must be a JSON object".into()))?;
    let mut out = Vec::with_capacity(obj.len());
    for (name, value) in obj {
        let name = name.to_ascii_lowercase();
        if name.trim().is_empty() {
            return Err(Error::InvalidInput("header names must not be empty".into()));
        }
        out.push(HeaderValue {
            name,
            value: parse_request_value(value, "header value")?,
        });
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(out)
}

fn parse_request_value(value: &Value, field: &str) -> Result<RequestValue> {
    if let Some(s) = value.as_str() {
        return Ok(RequestValue::Plain(s.to_string()));
    }
    if let Some(secret) = parse_secret(value)? {
        return Ok(RequestValue::Secret(secret));
    }
    Err(Error::InvalidInput(format!(
        "{field} must be a string or {{\"$secret\":\"name\"}}"
    )))
}

fn parse_body(value: Option<&Value>) -> Result<Option<RequestBody>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Some(s) = value.as_str() {
        return Ok(Some(RequestBody::Text(s.to_string())));
    }
    if let Some(secret) = parse_secret(value)? {
        return Ok(Some(RequestBody::Secret(secret)));
    }
    let obj = value.as_object().ok_or_else(|| {
        Error::InvalidInput("body must be a string, {\"$base64\":\"...\"}, or {\"$secret\":\"name\"}".into())
    })?;
    if obj.len() == 1 {
        if let Some(raw) = obj.get("$base64") {
            let raw = string_field(raw, "$base64")?;
            let bytes = B64
                .decode(raw)
                .map_err(|e| Error::InvalidInput(format!("body $base64 is invalid: {e}")))?;
            return Ok(Some(RequestBody::Base64(bytes)));
        }
    }
    Err(Error::InvalidInput(
        "body object must be {\"$base64\":\"...\"} or {\"$secret\":\"name\"}".into(),
    ))
}

fn parse_secret(value: &Value) -> Result<Option<String>> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if obj.len() == 1 {
        if let Some(secret) = obj.get("$secret") {
            let secret = string_field(secret, "$secret")?;
            if secret.trim().is_empty() {
                return Err(Error::InvalidInput("$secret name must not be empty".into()));
            }
            return Ok(Some(secret));
        }
    }
    Ok(None)
}

fn parse_timeout(value: Option<&Value>) -> Result<u64> {
    let Some(value) = value else {
        return Ok(DEFAULT_TIMEOUT_MS);
    };
    let timeout = value
        .as_u64()
        .ok_or_else(|| Error::InvalidInput("timeoutMs must be a positive integer".into()))?;
    if timeout == 0 || timeout > MAX_TIMEOUT_MS {
        return Err(Error::InvalidInput(format!(
            "timeoutMs must be between 1 and {MAX_TIMEOUT_MS}"
        )));
    }
    Ok(timeout)
}

fn parse_redirect(value: Option<&Value>) -> Result<RedirectPolicy> {
    let Some(value) = value else {
        return Ok(RedirectPolicy::Follow);
    };
    match string_field(value, "redirect")?.as_str() {
        "follow" => Ok(RedirectPolicy::Follow),
        "manual" => Ok(RedirectPolicy::Manual),
        "deny" => Ok(RedirectPolicy::Deny),
        other => Err(Error::InvalidInput(format!(
            "redirect must be follow, manual, or deny: {other}"
        ))),
    }
}

fn parse_response_body(value: Option<&Value>) -> Result<ResponseBodyMode> {
    let Some(value) = value else {
        return Ok(ResponseBodyMode::Auto);
    };
    match string_field(value, "responseBody")?.as_str() {
        "auto" => Ok(ResponseBodyMode::Auto),
        "inline" => Ok(ResponseBodyMode::Inline),
        "blob" => Ok(ResponseBodyMode::Blob),
        other => Err(Error::InvalidInput(format!(
            "responseBody must be auto, inline, or blob: {other}"
        ))),
    }
}

struct RequestJsonInput<'a> {
    method: &'a str,
    url: &'a str,
    headers: &'a [HeaderValue],
    body: &'a Option<RequestBody>,
    timeout_ms: u64,
    redirect: RedirectPolicy,
    response_body: ResponseBodyMode,
    sensitive: &'a BTreeSet<String>,
}

fn request_json(request: &RequestJsonInput<'_>, redact: bool) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "method".to_string(),
        Value::String(request.method.to_string()),
    );
    obj.insert("url".to_string(), Value::String(request.url.to_string()));
    let mut header_obj = Map::new();
    for header in request.headers {
        let value = if matches!(header.value, RequestValue::Secret(_)) {
            request_value_json(&header.value)
        } else if redact && is_sensitive_header(&header.name, request.sensitive) {
            Value::String(REDACTED.to_string())
        } else {
            request_value_json(&header.value)
        };
        header_obj.insert(header.name.clone(), value);
    }
    obj.insert("headers".to_string(), Value::Object(header_obj));
    if let Some(body) = request.body {
        obj.insert("body".to_string(), body_json(body));
    }
    obj.insert(
        "timeoutMs".to_string(),
        Value::Number(serde_json::Number::from(request.timeout_ms)),
    );
    obj.insert(
        "redirect".to_string(),
        Value::String(request.redirect.as_str().to_string()),
    );
    obj.insert(
        "responseBody".to_string(),
        Value::String(request.response_body.as_str().to_string()),
    );
    Value::Object(obj)
}

fn request_value_json(value: &RequestValue) -> Value {
    match value {
        RequestValue::Plain(value) => Value::String(value.clone()),
        RequestValue::Secret(name) => secret_json(name),
    }
}

fn body_json(body: &RequestBody) -> Value {
    match body {
        RequestBody::Text(value) => Value::String(value.clone()),
        RequestBody::Base64(bytes) => {
            let mut obj = Map::new();
            obj.insert("$base64".to_string(), Value::String(B64.encode(bytes)));
            Value::Object(obj)
        }
        RequestBody::Secret(name) => secret_json(name),
    }
}

fn secret_json(name: &str) -> Value {
    let mut obj = Map::new();
    obj.insert("$secret".to_string(), Value::String(name.to_string()));
    Value::Object(obj)
}

pub fn is_sensitive_header(name: &str, app_declared: &BTreeSet<String>) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key" | "api-key"
    ) || lower.ends_with("-token")
        || lower.ends_with("-secret")
        || app_declared.contains(&lower)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
