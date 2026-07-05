use std::collections::BTreeMap;

use serde_json::Value;
use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::cron::canonical_cron;
use crate::events::{cleared_event, fired_event, set_event};
use crate::types::{ScheduleKind, ScheduleSpec, SchedulerState};

pub const MAX_SCHEDULES_PER_APP: usize = 32;
pub const MAX_NAME_LEN: usize = 128;
pub const MAX_SPEC_JSON_LEN: usize = 4096;
pub const MAX_ARGS: usize = 16;

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    match name {
        "scheduler.set" => decide_set(ctx, args),
        "scheduler.clear" => decide_clear(ctx, args),
        "scheduler.fire" => decide_fire(ctx, args),
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn decide_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let spec_json = canonical_spec_json(arg(args, 2, "spec json")?)?;
    let state = state_ref::<SchedulerState>(ctx.state, "scheduler")?;
    let app_schedules = state.schedules.get(&app);
    if app_schedules.is_some_and(|schedules| {
        schedules.len() >= MAX_SCHEDULES_PER_APP && !schedules.contains_key(&name)
    }) {
        return Err(Error::InvalidInput(format!(
            "scheduler supports at most {MAX_SCHEDULES_PER_APP} schedules per app"
        )));
    }
    Ok(Decision::Commit(vec![set_event(&app, &name, &spec_json)?]))
}

fn decide_clear(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let exists = state_ref::<SchedulerState>(ctx.state, "scheduler")?
        .schedules
        .get(&app)
        .is_some_and(|schedules| schedules.contains_key(&name));
    if exists {
        Ok(Decision::Commit(vec![cleared_event(&app, &name)?]))
    } else {
        Ok(Decision::Commit(Vec::new()))
    }
}

fn decide_fire(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let scheduled_for = parse_u64(&arg(args, 2, "scheduled_for")?, "scheduled_for")?;
    let fired_at = parse_u64(&arg(args, 3, "fired_at")?, "fired_at")?;
    let skipped = parse_u64(&arg(args, 4, "skipped")?, "skipped")?;
    let exists = state_ref::<SchedulerState>(ctx.state, "scheduler")?
        .schedules
        .get(&app)
        .is_some_and(|schedules| schedules.contains_key(&name));
    if !exists {
        return Err(Error::InvalidInput(format!(
            "unknown scheduler schedule: {app}/{name}"
        )));
    }
    Ok(Decision::Commit(vec![fired_event(
        &app,
        &name,
        scheduled_for,
        fired_at,
        skipped,
    )?]))
}

pub(crate) fn parse_spec_json(raw: &str) -> Result<ScheduleSpec> {
    let canonical = canonical_spec_json(raw.to_string())?;
    let value: Value = serde_json::from_str(&canonical)
        .map_err(|e| Error::InvalidInput(format!("spec_json must be valid JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("spec_json must be a JSON object".into()))?;
    let kind = if let Some(at) = object.get("at") {
        ScheduleKind::At(
            at.as_u64()
                .ok_or_else(|| Error::InvalidInput("spec.at must be an epoch-ms integer".into()))?,
        )
    } else {
        let cron = object
            .get("cron")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidInput("spec.cron must be a string".into()))?;
        ScheduleKind::Cron(cron.to_string())
    };
    let verb = object
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or("timer")
        .to_string();
    let args = object
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| Error::InvalidInput("spec.args entries must be strings".into()))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(ScheduleSpec {
        kind,
        verb,
        args,
        spec_json: canonical,
    })
}

pub(crate) fn canonical_spec_json(raw: String) -> Result<String> {
    if raw.len() > MAX_SPEC_JSON_LEN {
        return Err(Error::InvalidInput(format!(
            "scheduler spec_json must be at most {MAX_SPEC_JSON_LEN} bytes"
        )));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| Error::InvalidInput(format!("spec_json must be valid JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("spec_json must be a JSON object".into()))?;
    let has_at = object.contains_key("at");
    let has_cron = object.contains_key("cron");
    if has_at == has_cron {
        return Err(Error::InvalidInput(
            "scheduler spec must contain exactly one of at or cron".into(),
        ));
    }

    let mut canonical = BTreeMap::new();
    if let Some(at) = object.get("at") {
        let at = at
            .as_u64()
            .ok_or_else(|| Error::InvalidInput("spec.at must be an epoch-ms integer".into()))?;
        canonical.insert("at".to_string(), Value::from(at));
    }
    if let Some(cron) = object.get("cron") {
        let cron = cron
            .as_str()
            .ok_or_else(|| Error::InvalidInput("spec.cron must be a string".into()))?;
        canonical.insert("cron".to_string(), Value::from(canonical_cron(cron)?));
    }

    let verb = match object.get("verb") {
        Some(value) => validate_verb(
            value
                .as_str()
                .ok_or_else(|| Error::InvalidInput("spec.verb must be a string".into()))?,
        )?,
        None => "timer".to_string(),
    };
    canonical.insert("verb".to_string(), Value::from(verb));

    let args = match object.get("args") {
        Some(value) => validate_args(value)?,
        None => Vec::new(),
    };
    canonical.insert(
        "args".to_string(),
        Value::Array(args.into_iter().map(Value::from).collect()),
    );
    serde_json::to_string(&canonical)
        .map_err(|e| Error::InvalidInput(format!("could not canonicalize scheduler spec: {e}")))
}

fn validate_name(name: String) -> Result<String> {
    if name.len() > MAX_NAME_LEN
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler name must use ASCII letters, digits, '.', '-' or '_', got {name:?}"
        )));
    }
    Ok(name)
}

fn validate_verb(verb: &str) -> Result<String> {
    if verb.is_empty()
        || verb.len() > MAX_NAME_LEN
        || verb.starts_with("__")
        || !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler verb must be a plain token and must not start with __, got {verb:?}"
        )));
    }
    Ok(verb.to_string())
}

fn validate_args(value: &Value) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("spec.args must be an array".into()))?;
    if values.len() > MAX_ARGS {
        return Err(Error::InvalidInput(format!(
            "scheduler spec.args must contain at most {MAX_ARGS} strings"
        )));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| Error::InvalidInput("spec.args entries must be strings".into()))
        })
        .collect()
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an unsigned integer")))
}
