use std::time::{SystemTime, UNIX_EPOCH};

use crate::{dispatch_on_core, invoke_app_input, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerFireOutcome {
    pub app: String,
    pub name: String,
    pub scheduled_for: u64,
    pub fired_at: u64,
    pub skipped: u64,
    pub verb: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn run_due(core: &mut HostCore) -> Result<Vec<SchedulerFireOutcome>, String> {
    run_due_at(core, now_epoch_ms()?)
}

pub fn run_due_at(
    core: &mut HostCore,
    tick_epoch_ms: u64,
) -> Result<Vec<SchedulerFireOutcome>, String> {
    let due = terrane_cap_scheduler::schedules_due_at(&core.state().scheduler, tick_epoch_ms)
        .map_err(|e| e.to_string())?;
    let mut outcomes = Vec::new();
    for schedule in due {
        dispatch_on_core(
            core,
            "scheduler.fire",
            &[
                schedule.app.clone(),
                schedule.name.clone(),
                schedule.scheduled_for.to_string(),
                tick_epoch_ms.to_string(),
                schedule.skipped.to_string(),
            ],
        )?;

        let mut input = Vec::with_capacity(schedule.args.len() + 3);
        input.push(schedule.verb.clone());
        input.push(schedule.name.clone());
        input.push(schedule.scheduled_for.to_string());
        input.extend(schedule.args.iter().cloned());
        let result = invoke_app_input(core, &schedule.app, &input);
        match result {
            Ok(output) => outcomes.push(SchedulerFireOutcome {
                app: schedule.app,
                name: schedule.name,
                scheduled_for: schedule.scheduled_for,
                fired_at: tick_epoch_ms,
                skipped: schedule.skipped,
                verb: schedule.verb,
                output: Some(output),
                error: None,
            }),
            Err(error) => outcomes.push(SchedulerFireOutcome {
                app: schedule.app,
                name: schedule.name,
                scheduled_for: schedule.scheduled_for,
                fired_at: tick_epoch_ms,
                skipped: schedule.skipped,
                verb: schedule.verb,
                output: None,
                error: Some(error),
            }),
        }
    }
    Ok(outcomes)
}

fn now_epoch_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "current time does not fit in u64 millis".to_string())
}
