use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::commands::parse_rule_json;
use crate::types::{AutomationState, FireStats, Fired, Removed, RuleEntry, Set, Suppressed};

pub fn set_event(app: &str, name: &str, rule_json: &str, rule_hash: &str) -> Result<EventRecord> {
    encode_event(
        "automation.set",
        &Set {
            app: app.to_string(),
            name: name.to_string(),
            rule_json: rule_json.to_string(),
            rule_hash: rule_hash.to_string(),
        },
    )
}

pub fn removed_event(app: &str, name: &str) -> Result<EventRecord> {
    encode_event(
        "automation.removed",
        &Removed {
            app: app.to_string(),
            name: name.to_string(),
        },
    )
}

pub fn fired_event(
    app: &str,
    name: &str,
    rule_hash: &str,
    event_ref: &str,
    fired_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "automation.fired",
        &Fired {
            app: app.to_string(),
            name: name.to_string(),
            rule_hash: rule_hash.to_string(),
            event_ref: event_ref.to_string(),
            fired_at,
        },
    )
}

pub fn suppressed_event(
    app: &str,
    name: &str,
    rule_hash: &str,
    event_ref: &str,
    suppressed_at: u64,
    reason: &str,
) -> Result<EventRecord> {
    encode_event(
        "automation.suppressed",
        &Suppressed {
            app: app.to_string(),
            name: name.to_string(),
            rule_hash: rule_hash.to_string(),
            event_ref: event_ref.to_string(),
            suppressed_at,
            reason: reason.to_string(),
        },
    )
}

pub fn decode_fired(record: &EventRecord) -> Result<FireStats> {
    let event: Fired = decode_event(record)?;
    Ok(FireStats {
        app: event.app,
        name: event.name,
        rule_hash: event.rule_hash,
        event_ref: event.event_ref,
        fired_at: event.fired_at,
    })
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "automation.set" => {
            let event: Set = decode_event(record)?;
            let spec = parse_rule_json(&event.rule_json)?;
            state_mut::<AutomationState>(state, "automation")?
                .rules
                .entry(event.app.clone())
                .or_default()
                .insert(
                    event.name.clone(),
                    RuleEntry {
                        app: event.app,
                        name: event.name,
                        spec,
                        rule_json: event.rule_json,
                        rule_hash: event.rule_hash,
                        last_fired_at: None,
                        fire_count: 0,
                        suppressed_count: 0,
                        seen_event_refs: Default::default(),
                    },
                );
        }
        "automation.removed" => {
            let event: Removed = decode_event(record)?;
            let state = state_mut::<AutomationState>(state, "automation")?;
            if let Some(rules) = state.rules.get_mut(&event.app) {
                rules.remove(&event.name);
                if rules.is_empty() {
                    state.rules.remove(&event.app);
                }
            }
        }
        "automation.fired" => {
            let event: Fired = decode_event(record)?;
            if let Some(rule) = state_mut::<AutomationState>(state, "automation")?
                .rules
                .get_mut(&event.app)
                .and_then(|rules| rules.get_mut(&event.name))
            {
                rule.last_fired_at = Some(event.fired_at);
                rule.fire_count = rule.fire_count.saturating_add(1);
                rule.seen_event_refs.insert(event.event_ref);
            }
        }
        "automation.suppressed" => {
            let event: Suppressed = decode_event(record)?;
            if let Some(rule) = state_mut::<AutomationState>(state, "automation")?
                .rules
                .get_mut(&event.app)
                .and_then(|rules| rules.get_mut(&event.name))
            {
                rule.suppressed_count = rule.suppressed_count.saturating_add(1);
                rule.seen_event_refs.insert(event.event_ref);
            }
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            state_mut::<AutomationState>(state, "automation")?
                .rules
                .remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "automation.set" => {
            let event: Set = decode_event(record).ok()?;
            Some(format!(
                "automation.set {}/{} hash={}",
                event.app, event.name, event.rule_hash
            ))
        }
        "automation.removed" => {
            let event: Removed = decode_event(record).ok()?;
            Some(format!("automation.removed {}/{}", event.app, event.name))
        }
        "automation.fired" => {
            let event: Fired = decode_event(record).ok()?;
            Some(format!(
                "automation.fired {}/{} event_ref={} fired_at={}",
                event.app, event.name, event.event_ref, event.fired_at
            ))
        }
        "automation.suppressed" => {
            let event: Suppressed = decode_event(record).ok()?;
            Some(format!(
                "automation.suppressed {}/{} event_ref={} reason={}",
                event.app, event.name, event.event_ref, event.reason
            ))
        }
        _ => None,
    }
}
