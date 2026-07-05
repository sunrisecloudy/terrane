use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::types::{
    Created, RunRecord, RunStarted, RunStatus, RunTerminal, ScheduleId, ScheduleRecord,
    SchedulerState,
};

pub fn created_event(
    app: &str,
    id: &str,
    cron: &str,
    timezone: &str,
    action: &str,
    payload_json: &str,
    next_due_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "scheduler.created",
        &Created {
            app: app.to_string(),
            id: id.to_string(),
            cron: cron.to_string(),
            timezone: timezone.to_string(),
            action: action.to_string(),
            payload_json: payload_json.to_string(),
            next_due_at,
        },
    )
}

pub fn paused_event(app: &str, id: &str) -> Result<EventRecord> {
    schedule_id_event("scheduler.paused", app, id)
}

pub fn resumed_event(app: &str, id: &str) -> Result<EventRecord> {
    schedule_id_event("scheduler.resumed", app, id)
}

pub fn removed_event(app: &str, id: &str) -> Result<EventRecord> {
    schedule_id_event("scheduler.removed", app, id)
}

pub fn run_started_event(
    app: &str,
    id: &str,
    run_id: &str,
    action: &str,
    payload_json: &str,
    due_at: u64,
    started_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "scheduler.run.started",
        &RunStarted {
            app: app.to_string(),
            id: id.to_string(),
            run_id: run_id.to_string(),
            action: action.to_string(),
            payload_json: payload_json.to_string(),
            due_at,
            started_at,
        },
    )
}

pub fn run_completed_event(
    app: &str,
    id: &str,
    run_id: &str,
    finished_at: u64,
    next_due_at: u64,
    output_json: &str,
) -> Result<EventRecord> {
    terminal_event(
        "scheduler.run.completed",
        app,
        id,
        run_id,
        finished_at,
        next_due_at,
        output_json,
    )
}

pub fn run_failed_event(
    app: &str,
    id: &str,
    run_id: &str,
    finished_at: u64,
    next_due_at: u64,
    error_json: &str,
) -> Result<EventRecord> {
    terminal_event(
        "scheduler.run.failed",
        app,
        id,
        run_id,
        finished_at,
        next_due_at,
        error_json,
    )
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "scheduler.created" => {
            let event: Created = decode_event(record)?;
            state_mut::<SchedulerState>(state, "scheduler")?
                .schedules
                .entry(event.app.clone())
                .or_default()
                .insert(
                    event.id.clone(),
                    ScheduleRecord {
                        id: event.id,
                        app: event.app,
                        cron: event.cron,
                        timezone: event.timezone,
                        action: event.action,
                        payload_json: event.payload_json,
                        paused: false,
                        next_due_at: event.next_due_at,
                        active_run_id: None,
                    },
                );
        }
        "scheduler.paused" => {
            let event: ScheduleId = decode_event(record)?;
            if let Some(schedule) = schedule_mut(state, &event.app, &event.id)? {
                schedule.paused = true;
            }
        }
        "scheduler.resumed" => {
            let event: ScheduleId = decode_event(record)?;
            if let Some(schedule) = schedule_mut(state, &event.app, &event.id)? {
                schedule.paused = false;
            }
        }
        "scheduler.removed" => {
            let event: ScheduleId = decode_event(record)?;
            let state = state_mut::<SchedulerState>(state, "scheduler")?;
            if let Some(schedules) = state.schedules.get_mut(&event.app) {
                schedules.remove(&event.id);
            }
        }
        "scheduler.run.started" => {
            let event: RunStarted = decode_event(record)?;
            let state = state_mut::<SchedulerState>(state, "scheduler")?;
            if let Some(schedule) = state
                .schedules
                .get_mut(&event.app)
                .and_then(|schedules| schedules.get_mut(&event.id))
            {
                if schedule.active_run_id.is_none() {
                    schedule.active_run_id = Some(event.run_id.clone());
                }
            }
            state.runs.entry(event.app.clone()).or_default().insert(
                event.run_id.clone(),
                RunRecord {
                    run_id: event.run_id,
                    schedule_id: event.id,
                    app: event.app,
                    action: event.action,
                    payload_json: event.payload_json,
                    status: RunStatus::Started,
                    due_at: event.due_at,
                    started_at: event.started_at,
                    finished_at: None,
                    output_json: None,
                    error_json: None,
                },
            );
        }
        "scheduler.run.completed" => {
            let event: RunTerminal = decode_event(record)?;
            apply_terminal(state, event, RunStatus::Completed)?;
        }
        "scheduler.run.failed" => {
            let event: RunTerminal = decode_event(record)?;
            apply_terminal(state, event, RunStatus::Failed)?;
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            let state = state_mut::<SchedulerState>(state, "scheduler")?;
            state.schedules.remove(&event.id);
            state.runs.remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "scheduler.created" => {
            let event: Created = decode_event(record).ok()?;
            Some(format!(
                "scheduler.created {}/{} cron=\"{}\" action={}",
                event.app, event.id, event.cron, event.action
            ))
        }
        "scheduler.paused" | "scheduler.resumed" | "scheduler.removed" => {
            let event: ScheduleId = decode_event(record).ok()?;
            Some(format!("{} {}/{}", record.kind, event.app, event.id))
        }
        "scheduler.run.started" => {
            let event: RunStarted = decode_event(record).ok()?;
            Some(format!(
                "scheduler.run.started {}/{} run={} due_at={}",
                event.app, event.id, event.run_id, event.due_at
            ))
        }
        "scheduler.run.completed" | "scheduler.run.failed" => {
            let event: RunTerminal = decode_event(record).ok()?;
            Some(format!(
                "{} {}/{} run={}",
                record.kind, event.app, event.id, event.run_id
            ))
        }
        _ => None,
    }
}

fn schedule_id_event(kind: &str, app: &str, id: &str) -> Result<EventRecord> {
    encode_event(
        kind,
        &ScheduleId {
            app: app.to_string(),
            id: id.to_string(),
        },
    )
}

fn terminal_event(
    kind: &str,
    app: &str,
    id: &str,
    run_id: &str,
    finished_at: u64,
    next_due_at: u64,
    payload_json: &str,
) -> Result<EventRecord> {
    encode_event(
        kind,
        &RunTerminal {
            app: app.to_string(),
            id: id.to_string(),
            run_id: run_id.to_string(),
            finished_at,
            next_due_at,
            payload_json: payload_json.to_string(),
        },
    )
}

fn schedule_mut<'a>(
    state: &'a mut dyn StateStore,
    app: &str,
    id: &str,
) -> Result<Option<&'a mut ScheduleRecord>> {
    Ok(state_mut::<SchedulerState>(state, "scheduler")?
        .schedules
        .get_mut(app)
        .and_then(|schedules| schedules.get_mut(id)))
}

fn apply_terminal(state: &mut dyn StateStore, event: RunTerminal, status: RunStatus) -> Result<()> {
    let state = state_mut::<SchedulerState>(state, "scheduler")?;
    if let Some(run) = state
        .runs
        .get_mut(&event.app)
        .and_then(|runs| runs.get_mut(&event.run_id))
    {
        if !run.status.is_terminal() {
            run.status = status;
            run.finished_at = Some(event.finished_at);
            match status {
                RunStatus::Completed => run.output_json = Some(event.payload_json.clone()),
                RunStatus::Failed => run.error_json = Some(event.payload_json.clone()),
                RunStatus::Started => {}
            }
        }
    }
    if let Some(schedule) = state
        .schedules
        .get_mut(&event.app)
        .and_then(|schedules| schedules.get_mut(&event.id))
    {
        if schedule.active_run_id.as_deref() == Some(&event.run_id) {
            schedule.active_run_id = None;
            schedule.next_due_at = event.next_due_at;
        }
    }
    Ok(())
}
