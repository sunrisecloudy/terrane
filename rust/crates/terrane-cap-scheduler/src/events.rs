use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::commands::parse_spec_json;
use crate::types::{Cleared, Fired, ScheduleEntry, Set, SchedulerState};

pub fn set_event(app: &str, name: &str, spec_json: &str) -> Result<EventRecord> {
    encode_event(
        "scheduler.set",
        &Set {
            app: app.to_string(),
            name: name.to_string(),
            spec_json: spec_json.to_string(),
        },
    )
}

pub fn cleared_event(app: &str, name: &str) -> Result<EventRecord> {
    encode_event(
        "scheduler.cleared",
        &Cleared {
            app: app.to_string(),
            name: name.to_string(),
        },
    )
}

pub fn fired_event(
    app: &str,
    name: &str,
    scheduled_for: u64,
    fired_at: u64,
    skipped: u64,
) -> Result<EventRecord> {
    encode_event(
        "scheduler.fired",
        &Fired {
            app: app.to_string(),
            name: name.to_string(),
            scheduled_for,
            fired_at,
            skipped,
        },
    )
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "scheduler.set" => {
            let event: Set = decode_event(record)?;
            let spec = parse_spec_json(&event.spec_json)?;
            state_mut::<SchedulerState>(state, "scheduler")?
                .schedules
                .entry(event.app.clone())
                .or_default()
                .insert(
                    event.name.clone(),
                    ScheduleEntry {
                        app: event.app,
                        name: event.name,
                        spec,
                        last_scheduled_for: None,
                        last_fired_at: None,
                        skipped_total: 0,
                    },
                );
        }
        "scheduler.cleared" => {
            let event: Cleared = decode_event(record)?;
            let state = state_mut::<SchedulerState>(state, "scheduler")?;
            if let Some(schedules) = state.schedules.get_mut(&event.app) {
                schedules.remove(&event.name);
                if schedules.is_empty() {
                    state.schedules.remove(&event.app);
                }
            }
        }
        "scheduler.fired" => {
            let event: Fired = decode_event(record)?;
            let state = state_mut::<SchedulerState>(state, "scheduler")?;
            let remove_after_fire = if let Some(schedule) = state
                .schedules
                .get_mut(&event.app)
                .and_then(|schedules| schedules.get_mut(&event.name))
            {
                schedule.last_scheduled_for = Some(event.scheduled_for);
                schedule.last_fired_at = Some(event.fired_at);
                schedule.skipped_total = schedule.skipped_total.saturating_add(event.skipped);
                schedule.spec.kind.is_one_shot()
            } else {
                false
            };
            if remove_after_fire {
                if let Some(schedules) = state.schedules.get_mut(&event.app) {
                    schedules.remove(&event.name);
                }
            }
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            state_mut::<SchedulerState>(state, "scheduler")?
                .schedules
                .remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "scheduler.set" => {
            let event: Set = decode_event(record).ok()?;
            Some(format!("scheduler.set {}/{}", event.app, event.name))
        }
        "scheduler.cleared" => {
            let event: Cleared = decode_event(record).ok()?;
            Some(format!("scheduler.cleared {}/{}", event.app, event.name))
        }
        "scheduler.fired" => {
            let event: Fired = decode_event(record).ok()?;
            Some(format!(
                "scheduler.fired {}/{} scheduled_for={} fired_at={} skipped={}",
                event.app, event.name, event.scheduled_for, event.fired_at, event.skipped
            ))
        }
        _ => None,
    }
}

impl crate::types::ScheduleKind {
    fn is_one_shot(&self) -> bool {
        matches!(self, Self::At(_))
    }
}
