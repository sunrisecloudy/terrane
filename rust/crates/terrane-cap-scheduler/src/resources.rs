use serde_json::json;
use terrane_cap_interface::{
    Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::cron::{latest_at_or_before, missed_since};
use crate::types::{DueSchedule, ScheduleEntry, ScheduleKind, SchedulerState};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "set",
            params: &["name", "specJson"],
        },
        ResourceMethod::Write {
            name: "clear",
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
            "unknown resource read: scheduler.{other}"
        ))),
    }
}

pub fn schedules_due_at(state: &SchedulerState, now_epoch_ms: u64) -> Result<Vec<DueSchedule>> {
    let mut due = Vec::new();
    for schedules in state.schedules.values() {
        for schedule in schedules.values() {
            if let Some(item) = due_for_schedule(schedule, now_epoch_ms)? {
                due.push(item);
            }
        }
    }
    due.sort_by(|a, b| {
        a.scheduled_for
            .cmp(&b.scheduled_for)
            .then_with(|| a.app.cmp(&b.app))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(due)
}

fn due_for_schedule(schedule: &ScheduleEntry, now_epoch_ms: u64) -> Result<Option<DueSchedule>> {
    match &schedule.spec.kind {
        ScheduleKind::At(at) => {
            if schedule.last_scheduled_for.is_none() && *at <= now_epoch_ms {
                Ok(Some(due(schedule, *at, 0)))
            } else {
                Ok(None)
            }
        }
        ScheduleKind::Cron(expr) => {
            if let Some(last) = schedule.last_scheduled_for {
                let missed = missed_since(expr, last, now_epoch_ms)?;
                if let Some(scheduled_for) = missed.last().copied() {
                    Ok(Some(due(
                        schedule,
                        scheduled_for,
                        missed.len().saturating_sub(1) as u64,
                    )))
                } else {
                    Ok(None)
                }
            } else if let Some(scheduled_for) = latest_at_or_before(expr, now_epoch_ms)? {
                Ok(Some(due(schedule, scheduled_for, 0)))
            } else {
                Ok(None)
            }
        }
    }
}

fn due(schedule: &ScheduleEntry, scheduled_for: u64, skipped: u64) -> DueSchedule {
    DueSchedule {
        app: schedule.app.clone(),
        name: schedule.name.clone(),
        scheduled_for,
        skipped,
        verb: schedule.spec.verb.clone(),
        args: schedule.spec.args.clone(),
    }
}

fn read_list(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let schedules = terrane_cap_interface::state_ref::<SchedulerState>(state, "scheduler")?
        .schedules
        .get(app)
        .cloned()
        .unwrap_or_default();
    Ok(ReadValue::StringMap(
        schedules
            .into_iter()
            .map(|(name, schedule)| (name, schedule_json(&schedule)))
            .collect(),
    ))
}

fn read_stat(state: &dyn StateStore, app: &str, name: &str) -> Result<ReadValue> {
    let value = terrane_cap_interface::state_ref::<SchedulerState>(state, "scheduler")?
        .schedules
        .get(app)
        .and_then(|schedules| schedules.get(name))
        .map(schedule_json);
    Ok(ReadValue::OptString(value))
}

fn schedule_json(schedule: &ScheduleEntry) -> String {
    json!({
        "app": schedule.app,
        "name": schedule.name,
        "spec": serde_json::from_str::<serde_json::Value>(&schedule.spec.spec_json)
            .unwrap_or(serde_json::Value::Null),
        "last_scheduled_for": schedule.last_scheduled_for,
        "last_fired_at": schedule.last_fired_at,
        "skipped_total": schedule.skipped_total,
    })
    .to_string()
}
