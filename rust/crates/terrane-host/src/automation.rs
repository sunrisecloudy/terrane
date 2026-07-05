use std::time::{SystemTime, UNIX_EPOCH};

use terrane_cap_automation::{event_json, matching_rules, render_args, PER_COMMIT_FIRE_BUDGET};
use terrane_core::{EventRecord, Request};

use crate::{invoke_app_input, CommandOutcome, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationTickOutcome {
    pub records: Vec<EventRecord>,
    pub backend_outputs: Vec<AutomationBackendOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationBackendOutcome {
    pub app: String,
    pub name: String,
    pub verb: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn run_tick(core: &mut HostCore) -> Result<AutomationTickOutcome, String> {
    run_tick_at(core, now_epoch_ms()?)
}

pub fn run_tick_at(
    core: &mut HostCore,
    tick_epoch_ms: u64,
) -> Result<AutomationTickOutcome, String> {
    let records = core.log_records().map_err(|e| e.to_string())?;
    process_records(core, &records, tick_epoch_ms)
}

pub fn process_records(
    core: &mut HostCore,
    records: &[EventRecord],
    tick_epoch_ms: u64,
) -> Result<AutomationTickOutcome, String> {
    let mut appended = Vec::new();
    let mut backend_outputs = Vec::new();
    let mut fired = 0usize;
    for record in records {
        if record.kind.starts_with("automation.") {
            continue;
        }
        let Some(match_event) = event_json(record).map_err(|e| e.to_string())? else {
            continue;
        };
        let rules = matching_rules(&core.state().automation, record, tick_epoch_ms)
            .map_err(|e| e.to_string())?;
        for rule in rules {
            if fired >= PER_COMMIT_FIRE_BUDGET {
                let args = vec![
                    rule.app.clone(),
                    rule.name.clone(),
                    rule.rule_hash.clone(),
                    match_event.event_ref.clone(),
                    tick_epoch_ms.to_string(),
                    "fire_budget_exceeded".to_string(),
                ];
                let records = core
                    .dispatch(Request::trusted_host("automation.suppress", args))
                    .map_err(|e| e.to_string())?;
                appended.extend(records);
                continue;
            }
            let args = vec![
                rule.app.clone(),
                rule.name.clone(),
                rule.rule_hash.clone(),
                match_event.event_ref.clone(),
                tick_epoch_ms.to_string(),
            ];
            let fire_records = core
                .dispatch(Request::trusted_host("automation.fire", args))
                .map_err(|e| e.to_string())?;
            if fire_records.is_empty() {
                continue;
            }
            appended.extend(fire_records);
            fired += 1;

            let mut input = Vec::with_capacity(rule.args_template.len() + 1);
            input.push(rule.verb.clone());
            input.extend(
                render_args(&rule.args_template, &match_event.event_json)
                    .map_err(|e| e.to_string())?,
            );
            match invoke_app_input(core, &rule.app, &input) {
                Ok(output) => backend_outputs.push(AutomationBackendOutcome {
                    app: rule.app,
                    name: rule.name,
                    verb: rule.verb,
                    output: Some(output),
                    error: None,
                }),
                Err(error) => backend_outputs.push(AutomationBackendOutcome {
                    app: rule.app,
                    name: rule.name,
                    verb: rule.verb,
                    output: None,
                    error: Some(error),
                }),
            }
        }
    }
    Ok(AutomationTickOutcome {
        records: appended,
        backend_outputs,
    })
}

pub fn command_outcome(outcome: AutomationTickOutcome) -> CommandOutcome {
    CommandOutcome {
        records: outcome.records,
        output: Some(format!(
            "automation tick: {} backend invocation(s)",
            outcome.backend_outputs.len()
        )),
    }
}

fn now_epoch_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "current time does not fit in u64 millis".to_string())
}
