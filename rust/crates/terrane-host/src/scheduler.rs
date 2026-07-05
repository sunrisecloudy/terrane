use std::time::{SystemTime, UNIX_EPOCH};

use crate::{dispatch_on_core, invoke_app_input, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerRunOutcome {
    pub app: String,
    pub schedule_id: String,
    pub run_id: String,
    pub action: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn run_due(core: &mut HostCore) -> Result<Vec<SchedulerRunOutcome>, String> {
    run_due_at(core, now_epoch_secs()?)
}

pub fn run_due_at(
    core: &mut HostCore,
    tick_epoch_secs: u64,
) -> Result<Vec<SchedulerRunOutcome>, String> {
    let due = terrane_cap_scheduler::schedules_due_at(&core.state().scheduler, tick_epoch_secs);
    let mut outcomes = Vec::new();
    for schedule in due {
        let run_id = format!("{}-{}-{}", schedule.app, schedule.id, tick_epoch_secs);
        dispatch_on_core(
            core,
            "scheduler.run.start",
            &[
                schedule.app.clone(),
                schedule.id.clone(),
                run_id.clone(),
                tick_epoch_secs.to_string(),
            ],
        )?;

        let input = vec![schedule.action.clone(), schedule.payload_json.clone()];
        let result = invoke_app_input(core, &schedule.app, &input);
        let finished_at = now_epoch_secs()?;
        match result {
            Ok(output) => {
                let output_json = serde_json::json!({ "output": output }).to_string();
                dispatch_on_core(
                    core,
                    "scheduler.run.complete",
                    &[
                        schedule.app.clone(),
                        schedule.id.clone(),
                        run_id.clone(),
                        finished_at.to_string(),
                        output_json,
                    ],
                )?;
                outcomes.push(SchedulerRunOutcome {
                    app: schedule.app,
                    schedule_id: schedule.id,
                    run_id,
                    action: schedule.action,
                    status: "completed".to_string(),
                    output: Some(output),
                    error: None,
                });
            }
            Err(error) => {
                let error_json = serde_json::json!({ "error": error }).to_string();
                dispatch_on_core(
                    core,
                    "scheduler.run.fail",
                    &[
                        schedule.app.clone(),
                        schedule.id.clone(),
                        run_id.clone(),
                        finished_at.to_string(),
                        error_json,
                    ],
                )?;
                outcomes.push(SchedulerRunOutcome {
                    app: schedule.app,
                    schedule_id: schedule.id,
                    run_id,
                    action: schedule.action,
                    status: "failed".to_string(),
                    output: None,
                    error: Some(error),
                });
            }
        }
    }
    Ok(outcomes)
}

fn now_epoch_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|e| e.to_string())
}
