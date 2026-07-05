use std::collections::BTreeSet;

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{Error, Result};

pub const DEFAULT_WAIT_MS: u64 = 2_000;
pub const MAX_WAIT_MS: u64 = 15_000;
pub const TOTAL_TIMEOUT_MS: u64 = 30_000;
pub const INLINE_AUTO_LIMIT: usize = 256 * 1024;
pub const INLINE_FORCED_LIMIT: usize = 8 * 1024 * 1024;
pub const BODY_HARD_LIMIT: usize = 32 * 1024 * 1024;
pub const MAX_VIEWPORT_W: u64 = 3_840;
pub const MAX_VIEWPORT_H: u64 = 2_160;
pub const DEFAULT_VIEWPORT_W: u64 = 1_280;
pub const DEFAULT_VIEWPORT_H: u64 = 800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRender {
    pub url: String,
    pub output: RenderOutput,
    pub wait_ms: u64,
    pub viewport_w: u64,
    pub viewport_h: u64,
    pub allowed_hosts: Vec<String>,
    pub canonical_json: String,
    pub redacted_json: String,
    pub request_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderOutput {
    Text,
    Html,
    Screenshot,
    Pdf,
}

impl RenderOutput {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderOutput::Text => "text",
            RenderOutput::Html => "html",
            RenderOutput::Screenshot => "screenshot",
            RenderOutput::Pdf => "pdf",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            RenderOutput::Text => "text/plain; charset=utf-8",
            RenderOutput::Html => "text/html; charset=utf-8",
            RenderOutput::Screenshot => "image/png",
            RenderOutput::Pdf => "application/pdf",
        }
    }

    pub fn is_blob_only(self) -> bool {
        matches!(self, RenderOutput::Screenshot | RenderOutput::Pdf)
    }
}

pub fn prepare_render(raw: &str) -> Result<PreparedRender> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("browser render request must be JSON object: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("browser render request must be a JSON object".into()))?;

    let url = obj
        .get("url")
        .ok_or_else(|| Error::InvalidInput("browser render request missing url".into()))
        .and_then(|value| string_field(value, "url"))?;
    if url.trim().is_empty() {
        return Err(Error::InvalidInput("url must not be empty".into()));
    }

    let output = parse_output(obj.get("output"))?;
    let wait_ms = parse_wait_ms(obj.get("waitMs"))?;
    let (viewport_w, viewport_h) = parse_viewport(obj.get("viewport"))?;
    let allowed_hosts = parse_string_list(obj.get("allowedHosts"), "allowedHosts")?;
    let sensitive = parse_string_set(obj.get("sensitiveHeaders"), "sensitiveHeaders")?;

    let request = RequestJsonInput {
        url: &url,
        output,
        wait_ms,
        viewport_w,
        viewport_h,
        allowed_hosts: &allowed_hosts,
        sensitive: &sensitive,
    };
    let canonical = request_json(&request, false);
    let redacted = request_json(&request, true);
    let canonical_json = serde_json::to_string(&canonical)
        .map_err(|e| Error::InvalidInput(format!("canonicalize browser render request: {e}")))?;
    let redacted_json = serde_json::to_string(&redacted)
        .map_err(|e| Error::InvalidInput(format!("redact browser render request: {e}")))?;
    let request_key = sha256_hex(canonical_json.as_bytes());

    Ok(PreparedRender {
        url,
        output,
        wait_ms,
        viewport_w,
        viewport_h,
        allowed_hosts,
        canonical_json,
        redacted_json,
        request_key,
    })
}

fn parse_output(value: Option<&Value>) -> Result<RenderOutput> {
    let Some(value) = value else {
        return Ok(RenderOutput::Text);
    };
    match string_field(value, "output")?.as_str() {
        "text" => Ok(RenderOutput::Text),
        "html" => Ok(RenderOutput::Html),
        "screenshot" => Ok(RenderOutput::Screenshot),
        "pdf" => Ok(RenderOutput::Pdf),
        other => Err(Error::InvalidInput(format!(
            "output must be text, html, screenshot, or pdf: {other}"
        ))),
    }
}

fn parse_wait_ms(value: Option<&Value>) -> Result<u64> {
    let Some(value) = value else {
        return Ok(DEFAULT_WAIT_MS);
    };
    let wait_ms = value
        .as_u64()
        .ok_or_else(|| Error::InvalidInput("waitMs must be a positive integer".into()))?;
    if wait_ms > MAX_WAIT_MS {
        return Err(Error::InvalidInput(format!(
            "waitMs must be between 0 and {MAX_WAIT_MS}"
        )));
    }
    Ok(wait_ms)
}

fn parse_viewport(value: Option<&Value>) -> Result<(u64, u64)> {
    let Some(value) = value else {
        return Ok((DEFAULT_VIEWPORT_W, DEFAULT_VIEWPORT_H));
    };
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("viewport must be an object".into()))?;
    let w = obj
        .get("w")
        .ok_or_else(|| Error::InvalidInput("viewport missing w".into()))
        .and_then(|value| positive_u64(value, "viewport.w"))?;
    let h = obj
        .get("h")
        .ok_or_else(|| Error::InvalidInput("viewport missing h".into()))
        .and_then(|value| positive_u64(value, "viewport.h"))?;
    if w > MAX_VIEWPORT_W || h > MAX_VIEWPORT_H {
        return Err(Error::InvalidInput(format!(
            "viewport must be at most {MAX_VIEWPORT_W}x{MAX_VIEWPORT_H}"
        )));
    }
    Ok((w, h))
}

fn parse_string_list(value: Option<&Value>, name: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let Some(value) = value else {
        return Ok(out);
    };
    let array = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be an array")))?;
    for item in array {
        let item = string_field(item, &format!("{name} item"))?.to_ascii_lowercase();
        if item.trim().is_empty() {
            return Err(Error::InvalidInput(format!("{name} items must not be empty")));
        }
        out.push(item);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn parse_string_set(value: Option<&Value>, name: &str) -> Result<BTreeSet<String>> {
    Ok(parse_string_list(value, name)?.into_iter().collect())
}

fn positive_u64(value: &Value, name: &str) -> Result<u64> {
    let value = value
        .as_u64()
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a positive integer")))?;
    if value == 0 {
        return Err(Error::InvalidInput(format!("{name} must be greater than zero")));
    }
    Ok(value)
}

fn string_field(value: &Value, name: &str) -> Result<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("{name} must be a string")))
}

struct RequestJsonInput<'a> {
    url: &'a str,
    output: RenderOutput,
    wait_ms: u64,
    viewport_w: u64,
    viewport_h: u64,
    allowed_hosts: &'a [String],
    sensitive: &'a BTreeSet<String>,
}

fn request_json(request: &RequestJsonInput<'_>, redact: bool) -> Value {
    let mut obj = Map::new();
    obj.insert("url".to_string(), Value::String(redact_url(request.url, redact)));
    obj.insert("output".to_string(), Value::String(request.output.as_str().to_string()));
    obj.insert(
        "waitMs".to_string(),
        Value::Number(serde_json::Number::from(request.wait_ms)),
    );
    let mut viewport = Map::new();
    viewport.insert(
        "w".to_string(),
        Value::Number(serde_json::Number::from(request.viewport_w)),
    );
    viewport.insert(
        "h".to_string(),
        Value::Number(serde_json::Number::from(request.viewport_h)),
    );
    obj.insert("viewport".to_string(), Value::Object(viewport));
    obj.insert(
        "allowedHosts".to_string(),
        Value::Array(
            request
                .allowed_hosts
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    obj.insert(
        "sensitiveHeaders".to_string(),
        Value::Array(request.sensitive.iter().cloned().map(Value::String).collect()),
    );
    Value::Object(obj)
}

fn redact_url(url: &str, redact: bool) -> String {
    if !redact {
        return url.to_string();
    }
    let without_fragment = url.split_once('#').map(|(left, _)| left).unwrap_or(url);
    match without_fragment.split_once('?') {
        Some((left, _)) => format!("{left}?<redacted>"),
        None => without_fragment.to_string(),
    }
}

pub fn host_and_path_without_query(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let without_fragment = after_scheme
        .split_once('#')
        .map(|(left, _)| left)
        .unwrap_or(after_scheme);
    let without_query = without_fragment
        .split_once('?')
        .map(|(left, _)| left)
        .unwrap_or(without_fragment);
    if without_query.is_empty() {
        "<url>".to_string()
    } else {
        without_query.to_string()
    }
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
