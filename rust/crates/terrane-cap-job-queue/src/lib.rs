//! The `job` capability — durable app-owned background jobs with retries.
//!
//! The queue is folded state, rebuilt from recorded lifecycle facts. The host
//! owns clocks and execution: it submits edge-minted ids/timestamps, records
//! starts/reports/reaps as trusted facts, and invokes app backends outside the
//! request path. Replay folds the job history and never re-runs work.
//!
//! Durable multi-step workflows compose this primitive with `scheduler`: a
//! scheduled backend verb can call `job.submit`, and later host ticks drain due
//! jobs. The scheduler records wake-up facts; this capability records job-run
//! facts; each attempted backend run records its own ordinary capability events.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{json, Value};
use terrane_cap_interface::{
    arg, command_doc, decode_app_removed, decode_event, encode_event, ensure_app_exists,
    event_doc, limit, non_empty, param, query_doc, resource_method, state_mut, state_ref,
    CapManifest, Capability, CapabilityDoc, CommandCtx, CommandSpec,
    Decision, Error, EventPattern, EventRecord, EventSpec, ExampleDoc, GrantResourceSpec,
    InternalNote, QueryCtx, QuerySpec, QueryValue, ReadValue, ResourceMethod, ResourceReadCtx,
    Result, StateStore,
};

mod doc;

pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
pub const DEFAULT_BASE_DELAY_MS: u64 = 1000;
pub const DEFAULT_FACTOR: u32 = 2;
pub const DEFAULT_MAX_DELAY_MS: u64 = 300_000;
pub const DEFAULT_LEASE_MS: u64 = 60_000;
pub const MAX_ATTEMPTS: u32 = 10;
pub const MAX_DELAY_MS: u64 = 3_600_000;
pub const MAX_ARGS_JSON_BYTES: usize = 16 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_NOTE_BYTES: usize = 1024;
pub const MAX_NON_TERMINAL_JOBS_PER_APP: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobState {
    pub jobs: BTreeMap<String, BTreeMap<String, Job>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub app: String,
    pub job_id: String,
    pub verb: String,
    pub args_json: String,
    pub retry: RetryPolicy,
    pub status: JobStatus,
    pub attempt: u32,
    pub progress_pct: Option<u8>,
    pub progress_note: Option<String>,
    pub submitted_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub cancelled_at: Option<u64>,
    pub next_attempt_at: Option<u64>,
    pub lease_until: Option<u64>,
    pub last_error: Option<String>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueJob {
    pub app: String,
    pub job_id: String,
    pub verb: String,
    pub args_json: String,
    pub attempt: u32,
    pub action: DueAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueAction {
    Start,
    Reap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub factor: u32,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            base_delay_ms: DEFAULT_BASE_DELAY_MS,
            factor: DEFAULT_FACTOR,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }
}

impl RetryPolicy {
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        backoff_ms(self.base_delay_ms, self.factor, self.max_delay_ms, attempt)
    }
}

pub fn backoff_ms(base_delay_ms: u64, factor: u32, max_delay_ms: u64, attempt: u32) -> u64 {
    let mut delay = base_delay_ms;
    for _ in 1..attempt {
        delay = delay.saturating_mul(u64::from(factor));
        if delay >= max_delay_ms {
            return max_delay_ms;
        }
    }
    delay.min(max_delay_ms)
}

pub fn parse_retry_json(raw: &str) -> Result<RetryPolicy> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(RetryPolicy::default());
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| Error::InvalidInput(format!("retry_json must be valid JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("retry_json must be a JSON object".into()))?;
    let policy = RetryPolicy {
        max_attempts: json_u32(object.get("maxAttempts"), DEFAULT_MAX_ATTEMPTS, "maxAttempts")?,
        base_delay_ms: json_u64(
            object.get("baseDelayMs"),
            DEFAULT_BASE_DELAY_MS,
            "baseDelayMs",
        )?,
        factor: json_u32(object.get("factor"), DEFAULT_FACTOR, "factor")?,
        max_delay_ms: json_u64(object.get("maxDelayMs"), DEFAULT_MAX_DELAY_MS, "maxDelayMs")?,
    };
    validate_retry(&policy)?;
    Ok(policy)
}

pub fn retry_json(policy: &RetryPolicy) -> String {
    json!({
        "maxAttempts": policy.max_attempts,
        "baseDelayMs": policy.base_delay_ms,
        "factor": policy.factor,
        "maxDelayMs": policy.max_delay_ms,
    })
    .to_string()
}

pub fn jobs_due_at(state: &JobState, now_ms: u64) -> Vec<DueJob> {
    let mut due = Vec::new();
    for jobs in state.jobs.values() {
        for job in jobs.values() {
            match job.status {
                JobStatus::Queued => {
                    if job.next_attempt_at.unwrap_or(job.submitted_at) <= now_ms {
                        due.push(DueJob {
                            app: job.app.clone(),
                            job_id: job.job_id.clone(),
                            verb: job.verb.clone(),
                            args_json: job.args_json.clone(),
                            attempt: job.attempt.saturating_add(1),
                            action: DueAction::Start,
                        });
                    }
                }
                JobStatus::Running => {
                    if job.lease_until.is_some_and(|lease| lease < now_ms) {
                        due.push(DueJob {
                            app: job.app.clone(),
                            job_id: job.job_id.clone(),
                            verb: job.verb.clone(),
                            args_json: job.args_json.clone(),
                            attempt: job.attempt,
                            action: DueAction::Reap,
                        });
                    }
                }
                JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled => {}
            }
        }
    }
    due.sort_by(|a, b| {
        action_rank(&a.action)
            .cmp(&action_rank(&b.action))
            .then_with(|| a.app.cmp(&b.app))
            .then_with(|| a.job_id.cmp(&b.job_id))
    });
    due
}

fn action_rank(action: &DueAction) -> u8 {
    match action {
        DueAction::Reap => 0,
        DueAction::Start => 1,
    }
}

pub struct JobQueueCapability;

impl Capability for JobQueueCapability {
    fn namespace(&self) -> &'static str {
        "job"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "job.submit" },
                CommandSpec { name: "job.cancel" },
                CommandSpec { name: "job.progress" },
                CommandSpec { name: "job.start" },
                CommandSpec { name: "job.report" },
                CommandSpec { name: "job.reap" },
            ],
            events: vec![
                EventSpec { kind: "job.submitted" },
                EventSpec { kind: "job.started" },
                EventSpec { kind: "job.progress" },
                EventSpec { kind: "job.completed" },
                EventSpec { kind: "job.failed" },
                EventSpec { kind: "job.stalled" },
                EventSpec { kind: "job.cancelled" },
            ],
            queries: vec![QuerySpec { name: "job.due" }],
            resources: resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "job",
                &["read", "write", "call"],
                "Background execution of this app's own backend verbs with retries.",
            )],
            subscriptions: vec![EventPattern { kind: "app.removed" }],
        }
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc {
        doc::job_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "job.submit" => decide_submit(ctx, args),
            "job.cancel" => decide_cancel(ctx, args),
            "job.progress" => decide_progress(ctx, args),
            "job.start" => decide_start(args),
            "job.report" => decide_report(ctx, args),
            "job.reap" => decide_reap(args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        describe(record)
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "due" => {
                let now_ms = parse_u64(&arg(args, 0, "now_ms")?, "now_ms")?;
                let state = state_ref::<JobState>(ctx.state, "job")?;
                Ok(QueryValue::Json(due_json(&jobs_due_at(state, now_ms))))
            }
            other => Err(Error::InvalidInput(format!("unknown query: job.{other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "stat" => {
                let job_id = args.first().map(String::as_str).unwrap_or_default();
                let value = state_ref::<JobState>(ctx.state, "job")?
                    .jobs
                    .get(ctx.app)
                    .and_then(|jobs| jobs.get(job_id))
                    .map(job_json);
                Ok(ReadValue::OptString(value))
            }
            "list" => {
                let status = args.first().filter(|s| !s.is_empty()).map(String::as_str);
                let jobs = state_ref::<JobState>(ctx.state, "job")?
                    .jobs
                    .get(ctx.app)
                    .cloned()
                    .unwrap_or_default();
                Ok(ReadValue::StringMap(
                    jobs.into_iter()
                        .filter(|(_, job)| status.is_none_or(|want| job.status.as_str() == want))
                        .map(|(id, job)| (id, job_json(&job)))
                        .collect(),
                ))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: job.{other}"
            ))),
        }
    }

    fn resource_api(&self) -> Vec<ResourceMethod> {
        resource_methods()
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        if method != "submit" {
            return Err(Error::InvalidInput(format!("job.{method} is not callable")));
        }
        let submitted = records
            .iter()
            .find(|record| record.kind == "job.submitted")
            .ok_or_else(|| Error::InvalidInput("job.submit recorded no job.submitted".into()))?;
        let event: Submitted = decode_event(submitted)?;
        Ok(ReadValue::OptString(Some(event.job_id)))
    }
}

fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Call {
            name: "submit",
            params: &["jobId", "verb", "argsJson", "retryJson", "submittedAt"],
        },
        ResourceMethod::Write {
            name: "cancel",
            params: &["jobId", "at"],
        },
        ResourceMethod::Write {
            name: "progress",
            params: &["jobId", "attempt", "pct", "note", "at", "leaseUntil"],
        },
        ResourceMethod::Read {
            name: "stat",
            params: &["jobId"],
        },
        ResourceMethod::Read {
            name: "list",
            params: &["status"],
        },
    ]
}

fn decide_submit(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let verb = validate_verb(non_empty(arg(args, 2, "verb")?, "verb")?)?;
    let args_json = canonical_args_json(&arg(args, 3, "args_json")?)?;
    let retry = parse_retry_json(&arg(args, 4, "retry_json")?)?;
    let submitted_at = parse_u64(&arg(args, 5, "submitted_at")?, "submitted_at")?;
    let state = state_ref::<JobState>(ctx.state, "job")?;
    let app_jobs = state.jobs.get(&app);
    if app_jobs.is_some_and(|jobs| jobs.contains_key(&job_id)) {
        return Err(Error::InvalidInput(format!(
            "job already exists: {app}/{job_id}"
        )));
    }
    let non_terminal = app_jobs
        .map(|jobs| {
            jobs.values()
                .filter(|job| !job.status.is_terminal())
                .count()
        })
        .unwrap_or(0);
    if non_terminal >= MAX_NON_TERMINAL_JOBS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "job supports at most {MAX_NON_TERMINAL_JOBS_PER_APP} non-terminal jobs per app"
        )));
    }
    Ok(Decision::Commit(vec![submitted_event(
        &app,
        &job_id,
        &verb,
        &args_json,
        &retry_json(&retry),
        submitted_at,
    )?]))
}

fn decide_cancel(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let at = parse_u64(&arg(args, 2, "at")?, "at")?;
    let Some(job) = state_ref::<JobState>(ctx.state, "job")?
        .jobs
        .get(&app)
        .and_then(|jobs| jobs.get(&job_id))
    else {
        return Ok(Decision::Commit(Vec::new()));
    };
    if job.status.is_terminal() {
        Ok(Decision::Commit(Vec::new()))
    } else {
        Ok(Decision::Commit(vec![cancelled_event(&app, &job_id, at)?]))
    }
}

fn decide_progress(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let attempt = parse_u32(&arg(args, 2, "attempt")?, "attempt")?;
    let pct = parse_pct(&arg(args, 3, "pct")?)?;
    let note = truncate_flag(&arg(args, 4, "note")?, MAX_NOTE_BYTES).0;
    let at = parse_u64(&arg(args, 5, "at")?, "at")?;
    let lease_until = parse_u64(&arg(args, 6, "lease_until")?, "lease_until")?;
    let Some(job) = state_ref::<JobState>(ctx.state, "job")?
        .jobs
        .get(&app)
        .and_then(|jobs| jobs.get(&job_id))
    else {
        return Err(Error::InvalidInput(format!("unknown job: {app}/{job_id}")));
    };
    if job.status != JobStatus::Running || job.attempt != attempt {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![progress_event(
        &app,
        &job_id,
        attempt,
        pct,
        &note,
        at,
        lease_until,
    )?]))
}

fn decide_start(args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let attempt = parse_u32(&arg(args, 2, "attempt")?, "attempt")?;
    let started_at = parse_u64(&arg(args, 3, "started_at")?, "started_at")?;
    let lease_until = parse_u64(&arg(args, 4, "lease_until")?, "lease_until")?;
    Ok(Decision::Commit(vec![started_event(
        &app,
        &job_id,
        attempt,
        started_at,
        lease_until,
    )?]))
}

fn decide_report(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let attempt = parse_u32(&arg(args, 2, "attempt")?, "attempt")?;
    let outcome = non_empty(arg(args, 3, "outcome")?, "outcome")?;
    let text = truncate_flag(&arg(args, 4, "output_or_error")?, MAX_OUTPUT_BYTES).0;
    let finished_at = parse_u64(&arg(args, 5, "finished_at")?, "finished_at")?;
    let next_attempt_at = parse_optional_u64(&arg(args, 6, "next_attempt_at")?, "next_attempt_at")?;
    let Some(job) = state_ref::<JobState>(ctx.state, "job")?
        .jobs
        .get(&app)
        .and_then(|jobs| jobs.get(&job_id))
    else {
        return Err(Error::InvalidInput(format!("unknown job: {app}/{job_id}")));
    };
    if job.status.is_terminal() || job.status != JobStatus::Running || job.attempt != attempt {
        return Ok(Decision::Commit(Vec::new()));
    }
    match outcome.as_str() {
        "completed" => Ok(Decision::Commit(vec![completed_event(
            &app,
            &job_id,
            attempt,
            &text,
            finished_at,
        )?])),
        "failed" => {
            if attempt < job.retry.max_attempts && next_attempt_at.is_none() {
                return Err(Error::InvalidInput(
                    "retryable job failure must include next_attempt_at".into(),
                ));
            }
            Ok(Decision::Commit(vec![failed_event(
                &app,
                &job_id,
                attempt,
                &text,
                finished_at,
                next_attempt_at,
            )?]))
        }
        _ => Err(Error::InvalidInput(
            "outcome must be completed or failed".into(),
        )),
    }
}

fn decide_reap(args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let job_id = validate_token("job_id", non_empty(arg(args, 1, "job_id")?, "job_id")?, 64)?;
    let at = parse_u64(&arg(args, 2, "at")?, "at")?;
    Ok(Decision::Commit(vec![stalled_event(&app, &job_id, at)?]))
}

pub fn submitted_event(
    app: &str,
    job_id: &str,
    verb: &str,
    args_json: &str,
    retry_json: &str,
    submitted_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "job.submitted",
        &Submitted {
            app: app.to_string(),
            job_id: job_id.to_string(),
            verb: verb.to_string(),
            args_json: args_json.to_string(),
            retry_json: retry_json.to_string(),
            submitted_at,
        },
    )
}

pub fn started_event(
    app: &str,
    job_id: &str,
    attempt: u32,
    started_at: u64,
    lease_until: u64,
) -> Result<EventRecord> {
    encode_event(
        "job.started",
        &Started {
            app: app.to_string(),
            job_id: job_id.to_string(),
            attempt,
            started_at,
            lease_until,
        },
    )
}

pub fn progress_event(
    app: &str,
    job_id: &str,
    attempt: u32,
    pct: u8,
    note: &str,
    at: u64,
    lease_until: u64,
) -> Result<EventRecord> {
    encode_event(
        "job.progress",
        &Progress {
            app: app.to_string(),
            job_id: job_id.to_string(),
            attempt,
            pct,
            note: note.to_string(),
            at,
            lease_until,
        },
    )
}

pub fn completed_event(
    app: &str,
    job_id: &str,
    attempt: u32,
    output: &str,
    finished_at: u64,
) -> Result<EventRecord> {
    encode_event(
        "job.completed",
        &Completed {
            app: app.to_string(),
            job_id: job_id.to_string(),
            attempt,
            output: output.to_string(),
            finished_at,
        },
    )
}

pub fn failed_event(
    app: &str,
    job_id: &str,
    attempt: u32,
    error: &str,
    finished_at: u64,
    next_attempt_at: Option<u64>,
) -> Result<EventRecord> {
    encode_event(
        "job.failed",
        &Failed {
            app: app.to_string(),
            job_id: job_id.to_string(),
            attempt,
            error: error.to_string(),
            finished_at,
            next_attempt_at,
        },
    )
}

pub fn stalled_event(app: &str, job_id: &str, at: u64) -> Result<EventRecord> {
    encode_event(
        "job.stalled",
        &Stalled {
            app: app.to_string(),
            job_id: job_id.to_string(),
            at,
        },
    )
}

pub fn cancelled_event(app: &str, job_id: &str, at: u64) -> Result<EventRecord> {
    encode_event(
        "job.cancelled",
        &Cancelled {
            app: app.to_string(),
            job_id: job_id.to_string(),
            at,
        },
    )
}

fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "job.submitted" => {
            let event: Submitted = decode_event(record)?;
            let retry = parse_retry_json(&event.retry_json)?;
            let job = Job {
                app: event.app.clone(),
                job_id: event.job_id.clone(),
                verb: event.verb,
                args_json: event.args_json,
                retry,
                status: JobStatus::Queued,
                attempt: 0,
                progress_pct: None,
                progress_note: None,
                submitted_at: event.submitted_at,
                started_at: None,
                finished_at: None,
                cancelled_at: None,
                next_attempt_at: Some(event.submitted_at),
                lease_until: None,
                last_error: None,
                output: None,
            };
            state_mut::<JobState>(state, "job")?
                .jobs
                .entry(event.app)
                .or_default()
                .insert(event.job_id, job);
        }
        "job.started" => {
            let event: Started = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if !job.status.is_terminal() {
                    job.status = JobStatus::Running;
                    job.attempt = event.attempt;
                    job.started_at = Some(event.started_at);
                    job.lease_until = Some(event.lease_until);
                    job.next_attempt_at = None;
                }
            }
        }
        "job.progress" => {
            let event: Progress = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if job.status == JobStatus::Running && job.attempt == event.attempt {
                    job.progress_pct = Some(event.pct);
                    job.progress_note = Some(event.note);
                    job.lease_until = Some(event.lease_until);
                }
            }
        }
        "job.completed" => {
            let event: Completed = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if !job.status.is_terminal() && job.attempt == event.attempt {
                    job.status = JobStatus::Done;
                    job.output = Some(event.output);
                    job.finished_at = Some(event.finished_at);
                    job.lease_until = None;
                    job.next_attempt_at = None;
                }
            }
        }
        "job.failed" => {
            let event: Failed = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if !job.status.is_terminal() && job.attempt == event.attempt {
                    job.last_error = Some(event.error);
                    job.finished_at = Some(event.finished_at);
                    job.lease_until = None;
                    if event.attempt < job.retry.max_attempts {
                        job.status = JobStatus::Queued;
                        job.next_attempt_at = event.next_attempt_at;
                    } else {
                        job.status = JobStatus::Failed;
                        job.next_attempt_at = None;
                    }
                }
            }
        }
        "job.stalled" => {
            let event: Stalled = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if job.status == JobStatus::Running {
                    job.last_error = Some("lease expired".to_string());
                    job.lease_until = None;
                    if job.attempt < job.retry.max_attempts {
                        job.status = JobStatus::Queued;
                        job.next_attempt_at = Some(event.at);
                    } else {
                        job.status = JobStatus::Failed;
                        job.finished_at = Some(event.at);
                        job.next_attempt_at = None;
                    }
                }
            }
        }
        "job.cancelled" => {
            let event: Cancelled = decode_event(record)?;
            if let Some(job) = job_mut(state, &event.app, &event.job_id)? {
                if !job.status.is_terminal() {
                    job.status = JobStatus::Cancelled;
                    job.cancelled_at = Some(event.at);
                    job.lease_until = None;
                    job.next_attempt_at = None;
                }
            }
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            state_mut::<JobState>(state, "job")?.jobs.remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

fn job_mut<'a>(
    state: &'a mut dyn StateStore,
    app: &str,
    job_id: &str,
) -> Result<Option<&'a mut Job>> {
    Ok(state_mut::<JobState>(state, "job")?
        .jobs
        .get_mut(app)
        .and_then(|jobs| jobs.get_mut(job_id)))
}

fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "job.submitted" => {
            let event: Submitted = decode_event(record).ok()?;
            Some(format!(
                "job.submitted {}/{} verb={}",
                event.app, event.job_id, event.verb
            ))
        }
        "job.started" => {
            let event: Started = decode_event(record).ok()?;
            Some(format!(
                "job.started {}/{} attempt={}",
                event.app, event.job_id, event.attempt
            ))
        }
        "job.progress" => {
            let event: Progress = decode_event(record).ok()?;
            Some(format!(
                "job.progress {}/{} attempt={} pct={}",
                event.app, event.job_id, event.attempt, event.pct
            ))
        }
        "job.completed" => {
            let event: Completed = decode_event(record).ok()?;
            Some(format!(
                "job.completed {}/{} attempt={}",
                event.app, event.job_id, event.attempt
            ))
        }
        "job.failed" => {
            let event: Failed = decode_event(record).ok()?;
            Some(format!(
                "job.failed {}/{} attempt={}",
                event.app, event.job_id, event.attempt
            ))
        }
        "job.stalled" => {
            let event: Stalled = decode_event(record).ok()?;
            Some(format!("job.stalled {}/{}", event.app, event.job_id))
        }
        "job.cancelled" => {
            let event: Cancelled = decode_event(record).ok()?;
            Some(format!("job.cancelled {}/{}", event.app, event.job_id))
        }
        _ => None,
    }
}

fn due_json(due: &[DueJob]) -> String {
    Value::Array(
        due.iter()
            .map(|job| {
                json!({
                    "action": match job.action {
                        DueAction::Start => "start",
                        DueAction::Reap => "reap",
                    },
                    "app": job.app,
                    "job_id": job.job_id,
                    "verb": job.verb,
                    "args_json": job.args_json,
                    "attempt": job.attempt,
                })
            })
            .collect(),
    )
    .to_string()
}

fn job_json(job: &Job) -> String {
    json!({
        "app": job.app,
        "job_id": job.job_id,
        "verb": job.verb,
        "args": serde_json::from_str::<Value>(&job.args_json).unwrap_or(Value::Null),
        "retry": {
            "maxAttempts": job.retry.max_attempts,
            "baseDelayMs": job.retry.base_delay_ms,
            "factor": job.retry.factor,
            "maxDelayMs": job.retry.max_delay_ms,
        },
        "status": job.status.as_str(),
        "attempt": job.attempt,
        "progress_pct": job.progress_pct,
        "progress_note": job.progress_note,
        "submitted_at": job.submitted_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
        "cancelled_at": job.cancelled_at,
        "next_attempt_at": job.next_attempt_at,
        "lease_until": job.lease_until,
        "last_error": job.last_error,
        "output": job.output,
    })
    .to_string()
}

fn canonical_args_json(raw: &str) -> Result<String> {
    if raw.len() > MAX_ARGS_JSON_BYTES {
        return Err(Error::InvalidInput(format!(
            "args_json must be at most {MAX_ARGS_JSON_BYTES} bytes"
        )));
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("args_json must be valid JSON: {e}")))?;
    if !value.is_array() {
        return Err(Error::InvalidInput(
            "args_json must be a JSON array of strings".into(),
        ));
    }
    let strings = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("args_json must be an array".into()))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| Error::InvalidInput("args_json entries must be strings".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    serde_json::to_string(&strings)
        .map_err(|e| Error::InvalidInput(format!("could not canonicalize args_json: {e}")))
}

fn validate_retry(policy: &RetryPolicy) -> Result<()> {
    if policy.max_attempts == 0 || policy.max_attempts > MAX_ATTEMPTS {
        return Err(Error::InvalidInput(format!(
            "retry.maxAttempts must be 1..={MAX_ATTEMPTS}"
        )));
    }
    if policy.base_delay_ms == 0 {
        return Err(Error::InvalidInput(
            "retry.baseDelayMs must be greater than zero".into(),
        ));
    }
    if policy.factor < 1 {
        return Err(Error::InvalidInput(
            "retry.factor must be at least 1".into(),
        ));
    }
    if policy.max_delay_ms > MAX_DELAY_MS {
        return Err(Error::InvalidInput(format!(
            "retry.maxDelayMs must be at most {MAX_DELAY_MS}"
        )));
    }
    Ok(())
}

fn validate_verb(verb: String) -> Result<String> {
    if verb.is_empty()
        || verb.len() > 128
        || verb.starts_with("__")
        || !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "job verb must be a plain token and must not start with __, got {verb:?}"
        )));
    }
    Ok(verb)
}

fn validate_token(label: &str, value: String, max_len: usize) -> Result<String> {
    if value.is_empty()
        || value.len() > max_len
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "{label} must use ASCII letters, digits, '.', '-' or '_', got {value:?}"
        )));
    }
    Ok(value)
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an unsigned integer")))
}

fn parse_optional_u64(raw: &str, label: &str) -> Result<Option<u64>> {
    if raw.trim().is_empty() || raw == "null" {
        Ok(None)
    } else {
        parse_u64(raw, label).map(Some)
    }
}

fn parse_u32(raw: &str, label: &str) -> Result<u32> {
    raw.parse::<u32>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an unsigned integer")))
}

fn parse_pct(raw: &str) -> Result<u8> {
    let value = raw
        .parse::<u8>()
        .map_err(|_| Error::InvalidInput("pct must be 0..=100".into()))?;
    if value > 100 {
        return Err(Error::InvalidInput("pct must be 0..=100".into()));
    }
    Ok(value)
}

fn json_u64(value: Option<&Value>, default: u64, label: &str) -> Result<u64> {
    value
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| Error::InvalidInput(format!("retry.{label} must be an integer")))
        })
        .unwrap_or(Ok(default))
}

fn json_u32(value: Option<&Value>, default: u32, label: &str) -> Result<u32> {
    let value = json_u64(value, u64::from(default), label)?;
    u32::try_from(value)
        .map_err(|_| Error::InvalidInput(format!("retry.{label} must fit in u32")))
}

fn truncate_flag(raw: &str, max_bytes: usize) -> (String, bool) {
    if raw.len() <= max_bytes {
        return (raw.to_string(), false);
    }
    let truncated = terrane_cap_interface::truncate(raw, max_bytes);
    (format!("{truncated}\n[truncated]"), true)
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Submitted {
    app: String,
    job_id: String,
    verb: String,
    args_json: String,
    retry_json: String,
    submitted_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Started {
    app: String,
    job_id: String,
    attempt: u32,
    started_at: u64,
    lease_until: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Progress {
    app: String,
    job_id: String,
    attempt: u32,
    pct: u8,
    note: String,
    at: u64,
    lease_until: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Completed {
    app: String,
    job_id: String,
    attempt: u32,
    output: String,
    finished_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Failed {
    app: String,
    job_id: String,
    attempt: u32,
    error: String,
    finished_at: u64,
    next_attempt_at: Option<u64>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Stalled {
    app: String,
    job_id: String,
    at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Cancelled {
    app: String,
    job_id: String,
    at: u64,
}

pub(crate) fn command_docs() -> Vec<terrane_cap_interface::CommandDoc> {
    vec![
        command_doc(
            "job.submit",
            &[
                param("app", "Existing app id.", "app_id"),
                param("job_id", "Edge-supplied sortable job id.", "token"),
                param("verb", "Backend verb to run.", "token"),
                param("args_json", "JSON string array passed after job id.", "json"),
                param("retry_json", "Retry policy JSON, or empty for defaults.", "json"),
                param("submitted_at", "Host-observed epoch milliseconds.", "epoch_ms"),
            ],
            "commit",
            "Queue one app-owned backend job.",
        )
        .with_emits(&["job.submitted"])
        .with_errors(&["app not found", "duplicate job", "invalid retry", "too many jobs"]),
        command_doc(
            "job.cancel",
            &[
                param("app", "Owning app id.", "app_id"),
                param("job_id", "Job id.", "token"),
                param("at", "Host-observed epoch milliseconds.", "epoch_ms"),
            ],
            "commit",
            "Cancel a non-terminal job.",
        )
        .with_errors(&["invalid job id", "invalid timestamp"])
        .with_emits(&["job.cancelled"]),
        command_doc(
            "job.progress",
            &[
                param("app", "Owning app id.", "app_id"),
                param("job_id", "Job id.", "token"),
                param("attempt", "Running attempt.", "integer"),
                param("pct", "Progress 0..100.", "integer"),
                param("note", "Short progress note.", "string"),
                param("at", "Host-observed epoch milliseconds.", "epoch_ms"),
                param("lease_until", "Renewed lease deadline.", "epoch_ms"),
            ],
            "commit",
            "Record progress for the current running attempt.",
        )
        .with_errors(&["unknown job", "invalid attempt", "pct must be 0..=100"])
        .with_emits(&["job.progress"]),
        command_doc(
            "job.start",
            &[
                param("app", "Owning app id.", "app_id"),
                param("job_id", "Job id.", "token"),
                param("attempt", "Attempt number.", "integer"),
                param("started_at", "Host-observed epoch milliseconds.", "epoch_ms"),
                param("lease_until", "Lease deadline.", "epoch_ms"),
            ],
            "commit",
            "Trusted host fact for starting an attempt.",
        )
        .with_errors(&["requires trusted host authority"])
        .with_emits(&["job.started"]),
        command_doc(
            "job.report",
            &[
                param("app", "Owning app id.", "app_id"),
                param("job_id", "Job id.", "token"),
                param("attempt", "Attempt number.", "integer"),
                param("outcome", "completed or failed.", "enum"),
                param("output_or_error", "Terminal output or error.", "string"),
                param("finished_at", "Host-observed epoch milliseconds.", "epoch_ms"),
                param("next_attempt_at", "Retry deadline or empty/null.", "epoch_ms?"),
            ],
            "commit",
            "Trusted host terminal report for one attempt.",
        )
        .with_errors(&["requires trusted host authority"])
        .with_emits(&["job.completed", "job.failed"]),
        command_doc(
            "job.reap",
            &[
                param("app", "Owning app id.", "app_id"),
                param("job_id", "Job id.", "token"),
                param("at", "Host-observed epoch milliseconds.", "epoch_ms"),
            ],
            "commit",
            "Trusted host fact for an expired running lease.",
        )
        .with_errors(&["requires trusted host authority"])
        .with_emits(&["job.stalled"]),
    ]
}

pub(crate) fn query_docs() -> Vec<terrane_cap_interface::QueryDoc> {
    vec![query_doc(
        "job.due",
        &[param("now_ms", "Caller-supplied epoch milliseconds.", "epoch_ms")],
        "JSON array of due start/reap items",
        "Pure host query for queued jobs due to start and running attempts whose lease expired.",
    )
    .with_errors(&["now_ms must be an unsigned integer"])]
}

pub(crate) fn event_docs() -> Vec<terrane_cap_interface::EventDoc> {
    vec![
        event_doc(
            "job.submitted",
            &[param("app", "Owning app id.", "app_id"), param("job_id", "Job id.", "token")],
            "Records one queued job.",
        ),
        event_doc("job.started", &[param("job_id", "Job id.", "token")], "Attempt started."),
        event_doc("job.progress", &[param("job_id", "Job id.", "token")], "Progress update."),
        event_doc("job.completed", &[param("job_id", "Job id.", "token")], "Attempt completed."),
        event_doc("job.failed", &[param("job_id", "Job id.", "token")], "Attempt failed."),
        event_doc("job.stalled", &[param("job_id", "Job id.", "token")], "Attempt lease expired."),
        event_doc("job.cancelled", &[param("job_id", "Job id.", "token")], "Job cancelled."),
    ]
}

pub(crate) fn resource_docs() -> Vec<terrane_cap_interface::ResourceMethodDoc> {
    let mut submit = resource_method(
        "submit",
        "call",
        &[
            param("jobId", "Edge-supplied job id.", "token"),
            param("verb", "Backend verb.", "token"),
            param("argsJson", "JSON string array.", "json"),
            param("retryJson", "Retry policy JSON.", "json"),
            param("submittedAt", "Host-observed epoch milliseconds.", "epoch_ms"),
        ],
        "Queue a job and return its id.",
    );
    submit.returns = "job id string".to_string();

    let mut cancel = resource_method(
        "cancel",
        "write",
        &[
            param("jobId", "Job id.", "token"),
            param("at", "Epoch ms.", "epoch_ms"),
        ],
        "Cancel a job.",
    );
    cancel.returns = "records job.cancelled when the job is non-terminal".to_string();

    let mut progress = resource_method(
        "progress",
        "write",
        &[param("jobId", "Job id.", "token")],
        "Record progress and renew the lease.",
    );
    progress.returns = "records job.progress for the current attempt".to_string();

    let mut stat = resource_method(
        "stat",
        "read",
        &[param("jobId", "Job id.", "token")],
        "Read one job state.",
    );
    stat.returns = "JSON job object or null".to_string();

    let mut list = resource_method(
        "list",
        "read",
        &[param("status", "Optional status.", "string")],
        "List jobs for this app.",
    );
    list.returns = "map of job id to JSON job object".to_string();

    vec![submit, cancel, progress, stat, list]
}

pub(crate) fn constraints() -> Vec<String> {
    vec![
        "The core never reads a clock and never runs a job during replay.".to_string(),
        "job.start, job.report, and job.reap are trusted-host-only lifecycle facts.".to_string(),
        "Terminal states win: later completion/failure/progress after cancel are folded as no-ops.".to_string(),
        "A stalled attempt counts toward maxAttempts and requeues immediately when attempts remain.".to_string(),
        "Scheduled workflows compose scheduler wake-ups with job.submit rather than adding job cron semantics.".to_string(),
    ]
}

pub(crate) fn limits() -> Vec<terrane_cap_interface::LimitDoc> {
    vec![
        limit("running", "2 per app / 8 per host", "Host worker concurrency gate."),
        limit("non_terminal", "1000 per app", "Enforced before job.submitted."),
        limit("args_json", "16 KiB", "JSON string array only."),
        limit("output/error", "64 KiB", "Truncated before event construction."),
        limit("note", "1 KiB", "Truncated before event construction."),
        limit("maxAttempts", "10", "Retry policy cap."),
        limit("maxDelayMs", "1 hour", "Retry policy cap."),
    ]
}

pub(crate) fn examples() -> Vec<ExampleDoc> {
    vec![ExampleDoc {
        title: "Submit a durable job".to_string(),
        summary: "Queue work from an app backend; a host tick later starts and reports it.".to_string(),
        language: "js".to_string(),
        code: "return ctx.resource.job.submit(jobId, 'send_digest', JSON.stringify(['daily']), JSON.stringify({ maxAttempts: 3 }), String(nowMs));".to_string(),
        expected: "records job.submitted; host records job.started and job.completed/job.failed".to_string(),
    }]
}

pub(crate) fn internal(include_internal: bool) -> Vec<InternalNote> {
    if include_internal {
        vec![InternalNote {
            title: "Worker contract".to_string(),
            body: "The host drains job.due(now_ms): reaps expired leases first, starts queued jobs within concurrency limits, invokes handle([verb, jobId, ...args]), then records job.report with host-observed times and computed retry backoff.".to_string(),
        }]
    } else {
        Vec::new()
    }
}
