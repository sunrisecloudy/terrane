use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{dispatch_on_core, invoke_app_input, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRunOutcome {
    pub app: String,
    pub job_id: String,
    pub attempt: u32,
    pub verb: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn run_due(core: &mut HostCore) -> Result<Vec<JobRunOutcome>, String> {
    run_due_at(core, now_epoch_ms()?)
}

pub fn run_due_at(core: &mut HostCore, tick_epoch_ms: u64) -> Result<Vec<JobRunOutcome>, String> {
    let due = terrane_cap_job_queue::jobs_due_at(&core.state().job, tick_epoch_ms);
    let mut outcomes = Vec::new();
    for item in due
        .iter()
        .filter(|item| item.action == terrane_cap_job_queue::DueAction::Reap)
    {
        dispatch_on_core(
            core,
            "job.reap",
            &[item.app.clone(), item.job_id.clone(), tick_epoch_ms.to_string()],
        )?;
    }

    let mut running_by_app = running_counts(core);
    let mut running_global = running_by_app.values().copied().sum::<usize>();
    for item in due
        .into_iter()
        .filter(|item| item.action == terrane_cap_job_queue::DueAction::Start)
    {
        if running_global >= 8 {
            break;
        }
        let app_running = running_by_app.get(&item.app).copied().unwrap_or(0);
        if app_running >= 2 {
            continue;
        }

        let started_at = tick_epoch_ms;
        let lease_until = started_at.saturating_add(terrane_cap_job_queue::DEFAULT_LEASE_MS);
        dispatch_on_core(
            core,
            "job.start",
            &[
                item.app.clone(),
                item.job_id.clone(),
                item.attempt.to_string(),
                started_at.to_string(),
                lease_until.to_string(),
            ],
        )?;
        running_global += 1;
        *running_by_app.entry(item.app.clone()).or_insert(0) += 1;

        let args = job_args(&item.args_json)?;
        let mut input = Vec::with_capacity(args.len() + 2);
        input.push(item.verb.clone());
        input.push(item.job_id.clone());
        input.extend(args);
        let result = invoke_app_input(core, &item.app, &input);
        let finished_at = now_epoch_ms().unwrap_or(tick_epoch_ms);
        match result {
            Ok(output) => {
                dispatch_on_core(
                    core,
                    "job.report",
                    &[
                        item.app.clone(),
                        item.job_id.clone(),
                        item.attempt.to_string(),
                        "completed".to_string(),
                        output.clone(),
                        finished_at.to_string(),
                        String::new(),
                    ],
                )?;
                outcomes.push(JobRunOutcome {
                    app: item.app,
                    job_id: item.job_id,
                    attempt: item.attempt,
                    verb: item.verb,
                    output: Some(output),
                    error: None,
                });
            }
            Err(error) => {
                let next_attempt_at = retry_at(core, &item.app, &item.job_id, item.attempt)
                    .map(|delay| finished_at.saturating_add(delay));
                dispatch_on_core(
                    core,
                    "job.report",
                    &[
                        item.app.clone(),
                        item.job_id.clone(),
                        item.attempt.to_string(),
                        "failed".to_string(),
                        error.clone(),
                        finished_at.to_string(),
                        next_attempt_at
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                    ],
                )?;
                outcomes.push(JobRunOutcome {
                    app: item.app,
                    job_id: item.job_id,
                    attempt: item.attempt,
                    verb: item.verb,
                    output: None,
                    error: Some(error),
                });
            }
        }
    }
    Ok(outcomes)
}

pub fn submit(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args_json: &str,
    retry_json: &str,
    submitted_at: u64,
    job_id: Option<&str>,
) -> Result<String, String> {
    let job_id = match job_id {
        Some(id) => id.to_string(),
        None => generate_job_id(submitted_at)?,
    };
    dispatch_on_core(
        core,
        "job.submit",
        &[
            app.to_string(),
            job_id.clone(),
            verb.to_string(),
            args_json.to_string(),
            retry_json.to_string(),
            submitted_at.to_string(),
        ],
    )?;
    Ok(job_id)
}

pub fn now_epoch_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "current time does not fit in u64 millis".to_string())
}

fn running_counts(core: &HostCore) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (app, jobs) in &core.state().job.jobs {
        let running = jobs
            .values()
            .filter(|job| job.status == terrane_cap_job_queue::JobStatus::Running)
            .count();
        if running > 0 {
            counts.insert(app.clone(), running);
        }
    }
    counts
}

fn retry_at(core: &HostCore, app: &str, job_id: &str, attempt: u32) -> Option<u64> {
    let job = core.state().job.jobs.get(app)?.get(job_id)?;
    (attempt < job.retry.max_attempts).then(|| job.retry.backoff_ms(attempt))
}

fn job_args(args_json: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(args_json).map_err(|e| e.to_string())?;
    value
        .as_array()
        .ok_or_else(|| "job args_json must be an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "job args_json entries must be strings".to_string())
        })
        .collect()
}

fn generate_job_id(epoch_ms: u64) -> Result<String, String> {
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).map_err(|e| format!("could not generate job id: {e}"))?;
    let mut id = format!("{epoch_ms:016x}");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").map_err(|e| e.to_string())?;
    }
    Ok(id)
}
