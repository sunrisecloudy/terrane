use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::cron::{next_due_after, validate_cron, validate_timezone};
use crate::events::{
    created_event, paused_event, removed_event, resumed_event, run_completed_event,
    run_failed_event, run_started_event,
};
use crate::types::{RunStatus, SchedulerState};

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    match name {
        "scheduler.create" => decide_create(ctx, args),
        "scheduler.pause" => decide_pause(ctx, args),
        "scheduler.resume" => decide_resume(ctx, args),
        "scheduler.remove" => decide_remove(ctx, args),
        "scheduler.run.start" => decide_run_start(ctx, args),
        "scheduler.run.complete" => decide_run_complete(ctx, args),
        "scheduler.run.fail" => decide_run_fail(ctx, args),
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn decide_create(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let id = validate_id(non_empty(arg(args, 1, "schedule id")?, "schedule id")?)?;
    let cron = non_empty(arg(args, 2, "cron")?, "cron")?;
    validate_cron(&cron)?;
    let timezone = non_empty(arg(args, 3, "timezone")?, "timezone")?;
    validate_timezone(&timezone)?;
    let action = validate_action(non_empty(arg(args, 4, "action")?, "action")?)?;
    let payload_json = valid_json(arg(args, 5, "payload json")?, "payload json")?;
    let next_due_at = args
        .get(6)
        .map(|raw| parse_u64(raw, "next due epoch seconds"))
        .transpose()?
        .unwrap_or(0);

    let state = state_ref::<SchedulerState>(ctx.state, "scheduler")?;
    if state
        .schedules
        .get(&app)
        .is_some_and(|schedules| schedules.contains_key(&id))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler schedule already exists: {app}/{id}"
        )));
    }
    Ok(Decision::Commit(vec![created_event(
        &app,
        &id,
        &cron,
        &timezone,
        &action,
        &payload_json,
        next_due_at,
    )?]))
}

fn decide_pause(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, id) = schedule_args(args)?;
    schedule(ctx, &app, &id)?;
    Ok(Decision::Commit(vec![paused_event(&app, &id)?]))
}

fn decide_resume(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, id) = schedule_args(args)?;
    schedule(ctx, &app, &id)?;
    Ok(Decision::Commit(vec![resumed_event(&app, &id)?]))
}

fn decide_remove(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, id) = schedule_args(args)?;
    let record = schedule(ctx, &app, &id)?;
    if record.active_run_id.is_some() {
        return Err(Error::InvalidInput(format!(
            "scheduler schedule has an active run: {app}/{id}"
        )));
    }
    Ok(Decision::Commit(vec![removed_event(&app, &id)?]))
}

fn decide_run_start(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, id) = schedule_args(args)?;
    let run_id = validate_id(non_empty(arg(args, 2, "run id")?, "run id")?)?;
    let now = parse_u64(&arg(args, 3, "now epoch seconds")?, "now epoch seconds")?;
    let record = schedule(ctx, &app, &id)?;
    if record.paused {
        return Err(Error::InvalidInput(format!(
            "scheduler schedule is paused: {app}/{id}"
        )));
    }
    if let Some(active) = &record.active_run_id {
        return Err(Error::InvalidInput(format!(
            "scheduler schedule already has active run: {app}/{id}/{active}"
        )));
    }
    if record.next_due_at > now {
        return Err(Error::InvalidInput(format!(
            "scheduler schedule is not due until {}: {app}/{id}",
            record.next_due_at
        )));
    }
    let state = state_ref::<SchedulerState>(ctx.state, "scheduler")?;
    if state
        .runs
        .get(&app)
        .is_some_and(|runs| runs.contains_key(&run_id))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler run already exists: {app}/{run_id}"
        )));
    }
    Ok(Decision::Commit(vec![run_started_event(
        &app,
        &id,
        &run_id,
        &record.action,
        &record.payload_json,
        record.next_due_at,
        now,
    )?]))
}

fn decide_run_complete(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    decide_run_terminal(ctx, args, true)
}

fn decide_run_fail(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    decide_run_terminal(ctx, args, false)
}

fn decide_run_terminal(ctx: CommandCtx<'_>, args: &[String], completed: bool) -> Result<Decision> {
    let (app, id) = schedule_args(args)?;
    let run_id = non_empty(arg(args, 2, "run id")?, "run id")?;
    let finished_at = parse_u64(
        &arg(args, 3, "finished epoch seconds")?,
        "finished epoch seconds",
    )?;
    let payload_json = valid_json(arg(args, 4, "payload json")?, "payload json")?;
    let record = schedule(ctx, &app, &id)?;
    let run = state_ref::<SchedulerState>(ctx.state, "scheduler")?
        .runs
        .get(&app)
        .and_then(|runs| runs.get(&run_id))
        .ok_or_else(|| Error::InvalidInput(format!("unknown scheduler run: {app}/{run_id}")))?;
    if run.schedule_id != id {
        return Err(Error::InvalidInput(format!(
            "scheduler run {app}/{run_id} belongs to schedule {}, not {id}",
            run.schedule_id
        )));
    }
    if run.status != RunStatus::Started {
        return Err(Error::InvalidInput(format!(
            "scheduler run is not active: {app}/{run_id} ({})",
            run.status.as_str()
        )));
    }
    let next_due_at = next_due_after(&record.cron, finished_at)?;
    let event = if completed {
        run_completed_event(&app, &id, &run_id, finished_at, next_due_at, &payload_json)?
    } else {
        run_failed_event(&app, &id, &run_id, finished_at, next_due_at, &payload_json)?
    };
    Ok(Decision::Commit(vec![event]))
}

fn schedule_args(args: &[String]) -> Result<(String, String)> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let id = non_empty(arg(args, 1, "schedule id")?, "schedule id")?;
    Ok((app, id))
}

fn schedule<'a>(
    ctx: CommandCtx<'a>,
    app: &str,
    id: &str,
) -> Result<&'a crate::types::ScheduleRecord> {
    state_ref::<SchedulerState>(ctx.state, "scheduler")?
        .schedules
        .get(app)
        .and_then(|schedules| schedules.get(id))
        .ok_or_else(|| Error::InvalidInput(format!("unknown scheduler schedule: {app}/{id}")))
}

fn valid_json(raw: String, label: &str) -> Result<String> {
    serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|e| Error::InvalidInput(format!("{label} must be valid JSON: {e}")))?;
    Ok(raw)
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an unsigned integer")))
}

fn validate_id(id: String) -> Result<String> {
    if id.len() > 96
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler id must use ASCII letters, digits, '.', '-' or '_', got {id:?}"
        )));
    }
    Ok(id)
}

fn validate_action(action: String) -> Result<String> {
    if action.len() > 96
        || !action
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "scheduler action must use ASCII letters, digits, '.', '-' or '_', got {action:?}"
        )));
    }
    Ok(action)
}
