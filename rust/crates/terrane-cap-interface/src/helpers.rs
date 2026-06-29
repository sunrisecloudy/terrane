use borsh::BorshDeserialize;

use crate::abi::{decode_event, Error, EventRecord, Result};
use crate::runtime::{CapBus, QueryValue};

pub fn app_exists(bus: &dyn CapBus, app: &str) -> Result<bool> {
    match bus.query("app", "exists", &[app.to_string()])? {
        QueryValue::Bool(value) => Ok(value),
        other => Err(Error::Runtime(format!(
            "app.exists returned unexpected value: {other:?}"
        ))),
    }
}

pub fn ensure_app_exists(bus: &dyn CapBus, app: &str) -> Result<()> {
    if app_exists(bus, app)? {
        Ok(())
    } else {
        Err(Error::AppNotFound(app.to_string()))
    }
}

pub fn replica_peer(bus: &dyn CapBus) -> Result<Option<u64>> {
    match bus.query("replica", "peer", &[])? {
        QueryValue::U64(peer) => Ok(peer),
        other => Err(Error::Runtime(format!(
            "replica.peer returned unexpected value: {other:?}"
        ))),
    }
}

/// Fetch a positional argument or fail with a clear message.
pub fn arg(args: &[String], index: usize, what: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| Error::InvalidInput(format!("missing {what}")))
}

/// Join trailing command args into one string value.
pub fn join_tail(args: &[String], from: usize) -> String {
    args.get(from..).unwrap_or_default().join(" ")
}

/// Join trailing command args and require the resulting string to be non-empty.
pub fn required_tail(args: &[String], from: usize, label: &str) -> Result<String> {
    non_empty(join_tail(args, from), label)
}

pub fn non_empty(raw: impl Into<String>, label: &str) -> Result<String> {
    let raw = raw.into();
    let value = raw.trim();
    if value.is_empty() {
        Err(Error::InvalidInput(format!("{label} must not be empty")))
    } else {
        Ok(value.to_string())
    }
}

pub fn non_empty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

pub fn parse_usize_arg(args: &[String], index: usize, what: &str) -> Result<usize> {
    let value = arg(args, index, what)?;
    value.parse::<usize>().map_err(|_| {
        Error::InvalidInput(format!(
            "{what} must be a non-negative integer, got {value:?}"
        ))
    })
}

#[derive(BorshDeserialize)]
pub struct AppRemoved {
    pub id: String,
}

pub fn decode_app_removed(record: &EventRecord) -> Result<AppRemoved> {
    decode_event(record)
}

pub fn extract_json_object<'a>(raw: &'a str, source: &str) -> Result<&'a str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed);
    }
    let start = raw
        .find('{')
        .ok_or_else(|| Error::InvalidInput(format!("{source} did not contain JSON")))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| Error::InvalidInput(format!("{source} did not contain complete JSON")))?;
    if end <= start {
        return Err(Error::InvalidInput(format!(
            "{source} JSON range is invalid"
        )));
    }
    Ok(&raw[start..=end])
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
}
