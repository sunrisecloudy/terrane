use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{delivered_event, failed_event, subscribed_event, unsubscribed_event};
use crate::types::{
    PushState, MAX_DETAIL_BYTES, MAX_PATTERN_BYTES, MAX_SUBSCRIPTIONS_PER_APP,
    MAX_TEMPLATE_BYTES,
};

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    match name {
        "push.subscribe" => decide_subscribe(ctx, args),
        "push.unsubscribe" => decide_unsubscribe(ctx, args),
        "push.record-delivery" => decide_record_delivery(args),
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn decide_subscribe(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let event_pattern = validate_pattern(&arg(args, 1, "event_pattern")?)?;
    let template = validate_template(&arg(args, 2, "template")?)?;
    let sub_id = match args.get(3) {
        Some(raw) => validate_sub_id(raw)?,
        None => derived_sub_id(&app, &event_pattern, &template),
    };
    let state = state_ref::<PushState>(ctx.state, "push")?;
    let existing = state.subscriptions.get(&app);
    let replaces_existing = existing.is_some_and(|subs| subs.contains_key(&sub_id));
    if !replaces_existing && existing.map(|subs| subs.len()).unwrap_or_default() >= MAX_SUBSCRIPTIONS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "push supports at most {MAX_SUBSCRIPTIONS_PER_APP} subscriptions per app"
        )));
    }
    Ok(Decision::Commit(vec![subscribed_event(
        &app,
        &sub_id,
        &event_pattern,
        &template,
    )?]))
}

fn decide_unsubscribe(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let sub_id = validate_sub_id(&arg(args, 1, "sub_id")?)?;
    Ok(Decision::Commit(vec![unsubscribed_event(&app, &sub_id)?]))
}

fn decide_record_delivery(args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let sub_id = validate_sub_id(&arg(args, 1, "sub_id")?)?;
    let event_seq = parse_event_seq(&arg(args, 2, "event_seq")?)?;
    let status = arg(args, 3, "status")?;
    match status.as_str() {
        "delivered" => Ok(Decision::Commit(vec![delivered_event(&app, &sub_id, event_seq)?])),
        "failed" => {
            let detail = validate_detail(args.get(4).map(String::as_str).unwrap_or("failed"))?;
            Ok(Decision::Commit(vec![failed_event(
                &app, &sub_id, event_seq, &detail,
            )?]))
        }
        _ => Err(Error::InvalidInput(
            "push delivery status must be delivered or failed".into(),
        )),
    }
}

pub fn validate_pattern(raw: &str) -> Result<String> {
    let value = non_empty(raw.to_string(), "event_pattern")?;
    if value.len() > MAX_PATTERN_BYTES {
        return Err(Error::InvalidInput(format!(
            "event_pattern exceeds {MAX_PATTERN_BYTES} bytes"
        )));
    }
    let ok = if let Some(ns) = value.strip_suffix(".*") {
        is_namespace(ns)
    } else {
        value.split_once('.').is_some_and(|(ns, tail)| {
            is_namespace(ns) && !tail.is_empty() && tail.bytes().all(is_kind_byte)
        })
    };
    if !ok {
        return Err(Error::InvalidInput(
            "event_pattern must be an exact kind like kv.set or namespace wildcard like kv.*".into(),
        ));
    }
    Ok(value)
}

pub fn validate_template(raw: &str) -> Result<String> {
    let value = non_empty(raw.to_string(), "template")?;
    if value.len() > MAX_TEMPLATE_BYTES {
        return Err(Error::InvalidInput(format!(
            "template exceeds {MAX_TEMPLATE_BYTES} bytes"
        )));
    }
    let mut open = false;
    for ch in value.chars() {
        match ch {
            '{' if open => return Err(Error::InvalidInput("template has nested placeholder".into())),
            '{' => open = true,
            '}' if !open => return Err(Error::InvalidInput("template has unmatched }".into())),
            '}' => open = false,
            _ => {}
        }
    }
    if open {
        return Err(Error::InvalidInput("template has unmatched {".into()));
    }
    Ok(value)
}

fn validate_sub_id(raw: &str) -> Result<String> {
    let value = non_empty(raw.to_string(), "sub_id")?;
    if value.len() > 96 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(Error::InvalidInput(
            "sub_id must use 1..96 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(value)
}

fn validate_detail(raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.len() > MAX_DETAIL_BYTES {
        return Err(Error::InvalidInput(format!("detail exceeds {MAX_DETAIL_BYTES} bytes")));
    }
    Ok(value.to_string())
}

fn parse_event_seq(raw: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput("event_seq must be a non-negative integer".into()))
}

fn is_namespace(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_kind_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
}

fn derived_sub_id(app: &str, pattern: &str, template: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(app.as_bytes());
    hasher.update(b"\0");
    hasher.update(pattern.as_bytes());
    hasher.update(b"\0");
    hasher.update(template.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::from("sub-");
    for byte in digest.iter().take(12) {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
