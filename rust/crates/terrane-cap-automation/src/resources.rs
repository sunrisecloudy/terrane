use serde_json::json;
use terrane_cap_interface::{
    Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::types::{AutomationState, RuleEntry};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "set",
            params: &["name", "ruleJson"],
        },
        ResourceMethod::Write {
            name: "rm",
            params: &["name"],
        },
        ResourceMethod::Read {
            name: "list",
            params: &[],
        },
        ResourceMethod::Read {
            name: "stat",
            params: &["name"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "list" => read_list(ctx.state, ctx.app),
        "stat" => {
            let name = args.first().map(String::as_str).unwrap_or_default();
            read_stat(ctx.state, ctx.app, name)
        }
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: automation.{other}"
        ))),
    }
}

pub fn read_list(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let rules = terrane_cap_interface::state_ref::<AutomationState>(state, "automation")?
        .rules
        .get(app)
        .cloned()
        .unwrap_or_default();
    Ok(ReadValue::StringMap(
        rules
            .into_iter()
            .map(|(name, rule)| (name, rule_json(&rule)))
            .collect(),
    ))
}

pub fn read_stat(state: &dyn StateStore, app: &str, name: &str) -> Result<ReadValue> {
    let value = terrane_cap_interface::state_ref::<AutomationState>(state, "automation")?
        .rules
        .get(app)
        .and_then(|rules| rules.get(name))
        .map(rule_json);
    Ok(ReadValue::OptString(value))
}

fn rule_json(rule: &RuleEntry) -> String {
    json!({
        "app": rule.app,
        "name": rule.name,
        "rule": serde_json::from_str::<serde_json::Value>(&rule.rule_json)
            .unwrap_or(serde_json::Value::Null),
        "rule_hash": rule.rule_hash,
        "last_fired_at": rule.last_fired_at,
        "fire_count": rule.fire_count,
        "suppressed_count": rule.suppressed_count,
    })
    .to_string()
}
