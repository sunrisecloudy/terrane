use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{fired_event, removed_event, set_event, suppressed_event};
use crate::types::{ActionSpec, AutomationState, RuleSpec, TriggerSpec};

pub const MAX_RULES_PER_APP: usize = 32;
pub const MAX_NAME_LEN: usize = 128;
pub const MAX_RULE_JSON_LEN: usize = 8192;
pub const MAX_ARGS_TEMPLATE: usize = 16;
pub const MIN_COOLDOWN_MS: u64 = 1000;

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    match name {
        "automation.set" => decide_set(ctx, args),
        "automation.rm" => decide_rm(ctx, args),
        "automation.fire" => decide_fire(ctx, args),
        "automation.suppress" => decide_suppress(ctx, args),
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn decide_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let rule_json = canonical_rule_json(ctx, &app, &arg(args, 2, "rule_json")?)?;
    let rule_hash = rule_hash(&rule_json);
    let state = state_ref::<AutomationState>(ctx.state, "automation")?;
    let app_rules = state.rules.get(&app);
    if app_rules.is_some_and(|rules| rules.len() >= MAX_RULES_PER_APP && !rules.contains_key(&name))
    {
        return Err(Error::InvalidInput(format!(
            "automation supports at most {MAX_RULES_PER_APP} rules per app"
        )));
    }
    Ok(Decision::Commit(vec![set_event(
        &app, &name, &rule_json, &rule_hash,
    )?]))
}

fn decide_rm(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let exists = state_ref::<AutomationState>(ctx.state, "automation")?
        .rules
        .get(&app)
        .is_some_and(|rules| rules.contains_key(&name));
    if exists {
        Ok(Decision::Commit(vec![removed_event(&app, &name)?]))
    } else {
        Ok(Decision::Commit(Vec::new()))
    }
}

fn decide_fire(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, name, rule_hash, event_ref, fired_at) = fire_args(args)?;
    let rule = state_ref::<AutomationState>(ctx.state, "automation")?
        .rules
        .get(&app)
        .and_then(|rules| rules.get(&name))
        .ok_or_else(|| Error::InvalidInput(format!("unknown automation rule: {app}/{name}")))?;
    if rule.rule_hash != rule_hash {
        return Err(Error::InvalidInput(format!(
            "stale automation rule hash for {app}/{name}"
        )));
    }
    if rule.seen_event_refs.contains(&event_ref) {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![fired_event(
        &app, &name, &rule_hash, &event_ref, fired_at,
    )?]))
}

fn decide_suppress(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, name, rule_hash, event_ref, suppressed_at) = fire_args(args)?;
    let reason = non_empty(arg(args, 5, "reason")?, "reason")?;
    let rule = state_ref::<AutomationState>(ctx.state, "automation")?
        .rules
        .get(&app)
        .and_then(|rules| rules.get(&name))
        .ok_or_else(|| Error::InvalidInput(format!("unknown automation rule: {app}/{name}")))?;
    if rule.rule_hash != rule_hash {
        return Err(Error::InvalidInput(format!(
            "stale automation rule hash for {app}/{name}"
        )));
    }
    if rule.seen_event_refs.contains(&event_ref) {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![suppressed_event(
        &app,
        &name,
        &rule_hash,
        &event_ref,
        suppressed_at,
        &reason,
    )?]))
}

fn fire_args(args: &[String]) -> Result<(String, String, String, String, u64)> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let name = validate_name(non_empty(arg(args, 1, "name")?, "name")?)?;
    let rule_hash = non_empty(arg(args, 2, "rule_hash")?, "rule_hash")?;
    let event_ref = non_empty(arg(args, 3, "event_ref")?, "event_ref")?;
    let fired_at = parse_u64(&arg(args, 4, "fired_at")?, "fired_at")?;
    Ok((app, name, rule_hash, event_ref, fired_at))
}

pub(crate) fn canonical_rule_json(
    ctx: CommandCtx<'_>,
    owning_app: &str,
    raw: &str,
) -> Result<String> {
    if raw.len() > MAX_RULE_JSON_LEN {
        return Err(Error::InvalidInput(format!(
            "automation rule_json must be at most {MAX_RULE_JSON_LEN} bytes"
        )));
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("rule_json must be valid JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("rule_json must be a JSON object".into()))?;
    let trigger = object
        .get("trigger")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidInput("rule.trigger must be an object".into()))?;
    let action = object
        .get("action")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidInput("rule.action must be an object".into()))?;

    let kind = trigger
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("rule.trigger.kind must be a string".into()))?;
    let kind = validate_kind(ctx, kind)?;
    let source_app = match trigger.get("sourceApp").and_then(Value::as_str) {
        Some(source) if !source.trim().is_empty() => Some(source.trim().to_string()),
        Some(_) => return Err(Error::InvalidInput("rule.trigger.sourceApp must not be empty".into())),
        None => None,
    };
    let effective_source = source_app.as_deref().unwrap_or(owning_app);
    ensure_app_exists(ctx.bus, effective_source)?;
    if effective_source != owning_app {
        let event_namespace = kind
            .split_once('.')
            .map(|(namespace, _)| namespace)
            .unwrap_or(kind.as_str());
        let allowed = terrane_cap_auth::namespace_granted(
            ctx.state,
            &terrane_cap_interface::ExecutionPrincipal::local_owner(),
            effective_source,
            event_namespace,
        )?;
        if !allowed {
            return Err(Error::InvalidInput(format!(
                "cross-app automation trigger requires grant {event_namespace} on {effective_source}"
            )));
        }
    }

    let filter = match trigger.get("filter").and_then(Value::as_str) {
        Some(filter) if !filter.trim().is_empty() => {
            terrane_cap_query::jmespath::eval(
                filter,
                &serde_json::json!({
                    "kind": kind,
                    "actor": "",
                    "payload": {
                        "app": effective_source,
                        "key": "",
                        "value": ""
                    }
                }),
            )?;
            Some(filter.trim().to_string())
        }
        Some(_) => return Err(Error::InvalidInput("rule.trigger.filter must not be empty".into())),
        None => None,
    };
    let verb = validate_verb(
        action
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidInput("rule.action.verb must be a string".into()))?,
    )?;
    let args_template = match action.get("argsTemplate") {
        Some(value) => validate_args_template(value)?,
        None => Vec::new(),
    };
    let cooldown_ms = match object.get("cooldownMs") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| Error::InvalidInput("rule.cooldownMs must be an integer".into()))?
            .max(MIN_COOLDOWN_MS),
        None => MIN_COOLDOWN_MS,
    };

    let mut canonical_trigger = BTreeMap::new();
    canonical_trigger.insert("kind".to_string(), Value::String(kind));
    if let Some(source_app) = source_app {
        canonical_trigger.insert("sourceApp".to_string(), Value::String(source_app));
    }
    if let Some(filter) = filter {
        canonical_trigger.insert("filter".to_string(), Value::String(filter));
    }
    let mut canonical_action = BTreeMap::new();
    canonical_action.insert("verb".to_string(), Value::String(verb));
    canonical_action.insert(
        "argsTemplate".to_string(),
        Value::Array(args_template.into_iter().map(Value::String).collect()),
    );
    let mut canonical = BTreeMap::new();
    canonical.insert(
        "trigger".to_string(),
        Value::Object(canonical_trigger.into_iter().collect()),
    );
    canonical.insert(
        "action".to_string(),
        Value::Object(canonical_action.into_iter().collect()),
    );
    canonical.insert("cooldownMs".to_string(), Value::from(cooldown_ms));
    serde_json::to_string(&canonical)
        .map_err(|e| Error::InvalidInput(format!("could not canonicalize automation rule: {e}")))
}

pub(crate) fn parse_rule_json(raw: &str) -> Result<RuleSpec> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("rule_json must be valid JSON: {e}")))?;
    let trigger = value
        .get("trigger")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidInput("rule.trigger must be an object".into()))?;
    let action = value
        .get("action")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidInput("rule.action must be an object".into()))?;
    let args_template = action
        .get("argsTemplate")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(RuleSpec {
        trigger: TriggerSpec {
            kind: trigger
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            source_app: trigger
                .get("sourceApp")
                .and_then(Value::as_str)
                .map(str::to_string),
            filter: trigger
                .get("filter")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        action: ActionSpec {
            verb: action
                .get("verb")
                .and_then(Value::as_str)
                .unwrap_or("automation")
                .to_string(),
            args_template,
        },
        cooldown_ms: value
            .get("cooldownMs")
            .and_then(Value::as_u64)
            .unwrap_or(MIN_COOLDOWN_MS),
    })
}

fn validate_kind(ctx: CommandCtx<'_>, raw: &str) -> Result<String> {
    let kind = raw.trim();
    if let Some(prefix) = kind.strip_suffix(".*") {
        if prefix.is_empty()
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Error::InvalidInput(format!(
                "automation trigger kind must be an event kind or namespace wildcard, got {raw:?}"
            )));
        }
        if !ctx.bus.event_kind_matches(kind) {
            return Err(Error::InvalidInput(format!(
                "automation trigger kind does not match any declared event: {raw}"
            )));
        }
        return Ok(kind.to_string());
    }
    if !kind.contains('.') || !ctx.bus.event_kind_matches(kind) {
        return Err(Error::InvalidInput(format!(
            "automation trigger kind must be a declared event kind, got {raw:?}"
        )));
    }
    Ok(kind.to_string())
}

fn validate_name(name: String) -> Result<String> {
    if name.len() > MAX_NAME_LEN
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "automation name must use ASCII letters, digits, '.', '-' or '_', got {name:?}"
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
            "automation verb must be a plain token and must not start with __, got {verb:?}"
        )));
    }
    Ok(verb.to_string())
}

fn validate_args_template(value: &Value) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("rule.action.argsTemplate must be an array".into()))?;
    if values.len() > MAX_ARGS_TEMPLATE {
        return Err(Error::InvalidInput(format!(
            "rule.action.argsTemplate must contain at most {MAX_ARGS_TEMPLATE} strings"
        )));
    }
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                Error::InvalidInput("rule.action.argsTemplate entries must be strings".into())
            })
        })
        .collect()
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an unsigned integer")))
}

pub(crate) fn rule_hash(rule_json: &str) -> String {
    let digest = Sha256::digest(rule_json.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
