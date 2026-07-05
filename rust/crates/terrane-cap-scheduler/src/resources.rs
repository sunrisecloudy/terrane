use terrane_cap_interface::{
    Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::types::{RunRecord, ScheduleRecord, SchedulerState};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "create",
            params: &["id", "cron", "timezone", "action", "payload"],
        },
        ResourceMethod::Read {
            name: "list",
            params: &[],
        },
        ResourceMethod::Write {
            name: "pause",
            params: &["id"],
        },
        ResourceMethod::Write {
            name: "resume",
            params: &["id"],
        },
        ResourceMethod::Write {
            name: "remove",
            params: &["id"],
        },
        ResourceMethod::Read {
            name: "history",
            params: &["id", "limit"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "list" => read_list(ctx.state, ctx.app),
        "history" => read_history(ctx.state, ctx.app, args),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: scheduler.{other}"
        ))),
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
            .map(|(id, schedule)| (id, schedule_json(&schedule)))
            .collect(),
    ))
}

fn read_history(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let schedule_id = args.first().map(String::as_str).unwrap_or_default();
    let limit = args
        .get(1)
        .map(|raw| parse_limit(raw))
        .transpose()?
        .unwrap_or(20);
    let mut runs: Vec<_> = terrane_cap_interface::state_ref::<SchedulerState>(state, "scheduler")?
        .runs
        .get(app)
        .map(|runs| {
            runs.values()
                .filter(|run| schedule_id.is_empty() || run.schedule_id == schedule_id)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    runs.sort_by_key(|run: &RunRecord| std::cmp::Reverse(run.started_at));
    Ok(ReadValue::StringList(
        runs.into_iter()
            .take(limit)
            .map(|run| run_json(&run))
            .collect(),
    ))
}

pub fn schedules_due_at(state: &SchedulerState, now_epoch_secs: u64) -> Vec<ScheduleRecord> {
    let mut due = Vec::new();
    for schedules in state.schedules.values() {
        for schedule in schedules.values() {
            if !schedule.paused
                && schedule.active_run_id.is_none()
                && schedule.next_due_at <= now_epoch_secs
            {
                due.push(schedule.clone());
            }
        }
    }
    due.sort_by(|a, b| {
        a.next_due_at
            .cmp(&b.next_due_at)
            .then_with(|| a.app.cmp(&b.app))
            .then_with(|| a.id.cmp(&b.id))
    });
    due
}

fn schedule_json(schedule: &ScheduleRecord) -> String {
    serde_json::json!({
        "id": schedule.id,
        "app": schedule.app,
        "cron": schedule.cron,
        "timezone": schedule.timezone,
        "action": schedule.action,
        "payload": serde_json::from_str::<serde_json::Value>(&schedule.payload_json).unwrap_or(serde_json::Value::Null),
        "paused": schedule.paused,
        "nextDueAt": schedule.next_due_at,
        "activeRunId": schedule.active_run_id,
    })
    .to_string()
}

fn run_json(run: &RunRecord) -> String {
    serde_json::json!({
        "runId": run.run_id,
        "scheduleId": run.schedule_id,
        "app": run.app,
        "action": run.action,
        "payload": serde_json::from_str::<serde_json::Value>(&run.payload_json).unwrap_or(serde_json::Value::Null),
        "status": run.status.as_str(),
        "dueAt": run.due_at,
        "startedAt": run.started_at,
        "finishedAt": run.finished_at,
        "output": run.output_json.as_deref().and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()),
        "error": run.error_json.as_deref().and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()),
    })
    .to_string()
}

fn parse_limit(raw: &str) -> Result<usize> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| Error::InvalidInput(format!("history limit must be numeric, got {raw:?}")))?;
    Ok(value.clamp(1, 100))
}
