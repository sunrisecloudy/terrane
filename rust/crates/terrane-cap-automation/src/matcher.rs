use serde_json::Value;
use sha2::{Digest, Sha256};
use terrane_cap_interface::{Error, EventRecord, Result};

use crate::commands::MIN_COOLDOWN_MS;
use crate::types::{AutomationState, MatchEvent, MatchingRule};

pub const PER_COMMIT_FIRE_BUDGET: usize = 8;

pub fn event_ref(record: &EventRecord) -> String {
    let mut hasher = Sha256::new();
    hasher.update(record.kind.as_bytes());
    hasher.update([0]);
    hasher.update(record.actor.as_bytes());
    hasher.update([0]);
    hasher.update(&record.payload);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn event_json(record: &EventRecord) -> Result<Option<MatchEvent>> {
    let Some(payload) = event_payload_json(record)? else {
        return Ok(None);
    };
    Ok(Some(MatchEvent {
        event_ref: event_ref(record),
        event_json: serde_json::json!({
            "kind": record.kind,
            "actor": record.actor,
            "payload": payload,
        }),
    }))
}

pub fn matching_rules(
    state: &AutomationState,
    record: &EventRecord,
    fired_at: u64,
) -> Result<Vec<MatchingRule>> {
    let Some(match_event) = event_json(record)? else {
        return Ok(Vec::new());
    };
    let payload_app = match_event
        .event_json
        .get("payload")
        .and_then(|payload| payload.get("app"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut matches = Vec::new();
    for rules in state.rules.values() {
        for rule in rules.values() {
            let source_app = rule.spec.trigger.source_app.as_deref().unwrap_or(&rule.app);
            if source_app != payload_app || !kind_matches(&rule.spec.trigger.kind, &record.kind) {
                continue;
            }
            if rule.seen_event_refs.contains(&match_event.event_ref) {
                continue;
            }
            let cooldown = rule.spec.cooldown_ms.max(MIN_COOLDOWN_MS);
            if rule
                .last_fired_at
                .is_some_and(|last| fired_at.saturating_sub(last) < cooldown)
            {
                continue;
            }
            if let Some(filter) = &rule.spec.trigger.filter {
                let result = terrane_cap_query::jmespath::eval(filter, &match_event.event_json)?;
                if result != "true" {
                    continue;
                }
            }
            matches.push(MatchingRule {
                app: rule.app.clone(),
                name: rule.name.clone(),
                rule_hash: rule.rule_hash.clone(),
                verb: rule.spec.action.verb.clone(),
                args_template: rule.spec.action.args_template.clone(),
            });
        }
    }
    Ok(matches)
}

pub fn render_args(args_template: &[String], event_json: &Value) -> Result<Vec<String>> {
    args_template
        .iter()
        .map(|template| render_template(template, event_json))
        .collect()
}

fn render_template(template: &str, event_json: &Value) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let (before, after_start) = rest.split_at(start);
        out.push_str(before);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            return Err(Error::InvalidInput(format!(
                "unterminated automation template expression in {template:?}"
            )));
        };
        let expr = after_start[..end].trim();
        out.push_str(&template_value(expr, event_json)?);
        rest = &after_start[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn template_value(expr: &str, event_json: &Value) -> Result<String> {
    if expr.is_empty() {
        return Err(Error::InvalidInput(
            "automation template expression must not be empty".into(),
        ));
    }
    let mut current = event_json;
    for part in expr.split('.') {
        current = current.get(part).ok_or_else(|| {
            Error::InvalidInput(format!("automation template field not found: {expr}"))
        })?;
    }
    Ok(match current {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => other.to_string(),
    })
}

fn kind_matches(pattern: &str, kind: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return kind
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'));
    }
    pattern == kind
}

fn event_payload_json(record: &EventRecord) -> Result<Option<Value>> {
    match record.kind.as_str() {
        "kv.set" | "kv.deleted" => terrane_cap_kv_payload(record),
        _ => Ok(None),
    }
}

fn terrane_cap_kv_payload(record: &EventRecord) -> Result<Option<Value>> {
    terrane_cap_kv::event_payload_json(record)
}
