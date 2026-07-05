use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, EventRecord, QueryValue, StateStore,
};
use terrane_cap_job_queue::{
    backoff_ms, jobs_due_at, DueAction, JobQueueCapability, JobState, JobStatus,
};

#[derive(Default)]
struct TestState {
    job: JobState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        (namespace == "job").then_some(&self.job as &dyn std::any::Any)
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        (namespace == "job").then_some(&mut self.job as &mut dyn std::any::Any)
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        if cap == "app" && name == "exists" {
            Ok(QueryValue::Bool(true))
        } else {
            Ok(QueryValue::Bool(false))
        }
    }
}

#[derive(borsh::BorshSerialize)]
struct AppRemoved {
    id: String,
}

fn commit(
    cap: &JobQueueCapability,
    state: &mut TestState,
    bus: &Bus,
    name: &str,
    args: &[&str],
) -> Vec<EventRecord> {
    let args = args.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
    let records = match cap
        .decide(
            CommandCtx {
                state: &*state,
                bus,
            },
            name,
            &args,
        )
        .unwrap()
    {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    for record in &records {
        cap.fold(state, record).unwrap();
    }
    records
}

fn submit(cap: &JobQueueCapability, state: &mut TestState, bus: &Bus, job_id: &str) {
    commit(
        cap,
        state,
        bus,
        "job.submit",
        &["ops", job_id, "work", r#"["payload"]"#, "", "1000"],
    );
}

#[test]
fn retry_backoff_is_capped_exponential() {
    assert_eq!(backoff_ms(1000, 2, 300_000, 1), 1000);
    assert_eq!(backoff_ms(1000, 2, 300_000, 3), 4000);
    assert_eq!(backoff_ms(1000, 10, 30_000, 6), 30_000);
}

#[test]
fn lifecycle_folds_and_replays_identically() {
    let cap = JobQueueCapability;
    let bus = Bus;
    let mut state = TestState::default();
    let mut log = Vec::new();
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "job.submit",
        &[
            "ops",
            "job-1",
            "work",
            r#"["payload"]"#,
            r#"{"maxAttempts":3,"baseDelayMs":1000,"factor":2,"maxDelayMs":300000}"#,
            "1000",
        ],
    ));
    assert_eq!(state.job.jobs["ops"]["job-1"].status, JobStatus::Queued);
    assert_eq!(jobs_due_at(&state.job, 1000)[0].action, DueAction::Start);

    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "job.start",
        &["ops", "job-1", "1", "1001", "61001"],
    ));
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "job.progress",
        &["ops", "job-1", "1", "50", "half", "2000", "62000"],
    ));
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "job.report",
        &["ops", "job-1", "1", "completed", "ok", "3000", ""],
    ));
    let job = &state.job.jobs["ops"]["job-1"];
    assert_eq!(job.status, JobStatus::Done);
    assert_eq!(job.output.as_deref(), Some("ok"));
    assert_eq!(job.progress_pct, Some(50));

    let mut replayed = TestState::default();
    for record in &log {
        cap.fold(&mut replayed, record).unwrap();
    }
    assert_eq!(replayed.job, state.job);
}

#[test]
fn failed_attempt_requeues_when_retry_due_and_stall_counts_toward_attempts() {
    let cap = JobQueueCapability;
    let bus = Bus;
    let mut state = TestState::default();
    commit(
        &cap,
        &mut state,
        &bus,
        "job.submit",
        &[
            "ops",
            "job-1",
            "work",
            "[]",
            r#"{"maxAttempts":2,"baseDelayMs":100,"factor":2,"maxDelayMs":1000}"#,
            "1000",
        ],
    );
    commit(
        &cap,
        &mut state,
        &bus,
        "job.start",
        &["ops", "job-1", "1", "1000", "2000"],
    );
    commit(
        &cap,
        &mut state,
        &bus,
        "job.report",
        &["ops", "job-1", "1", "failed", "boom", "1100", "1200"],
    );
    assert_eq!(state.job.jobs["ops"]["job-1"].status, JobStatus::Queued);
    assert!(jobs_due_at(&state.job, 1199).is_empty());
    assert_eq!(jobs_due_at(&state.job, 1200)[0].attempt, 2);

    commit(
        &cap,
        &mut state,
        &bus,
        "job.start",
        &["ops", "job-1", "2", "1200", "1300"],
    );
    assert_eq!(jobs_due_at(&state.job, 1301)[0].action, DueAction::Reap);
    commit(&cap, &mut state, &bus, "job.reap", &["ops", "job-1", "1301"]);
    assert_eq!(state.job.jobs["ops"]["job-1"].status, JobStatus::Failed);
    assert_eq!(
        state.job.jobs["ops"]["job-1"].last_error.as_deref(),
        Some("lease expired")
    );
}

#[test]
fn cancellation_wins_over_late_terminal_events() {
    let cap = JobQueueCapability;
    let bus = Bus;
    let mut state = TestState::default();
    submit(&cap, &mut state, &bus, "job-1");
    commit(
        &cap,
        &mut state,
        &bus,
        "job.start",
        &["ops", "job-1", "1", "1000", "2000"],
    );
    commit(&cap, &mut state, &bus, "job.cancel", &["ops", "job-1", "1500"]);
    commit(
        &cap,
        &mut state,
        &bus,
        "job.report",
        &["ops", "job-1", "1", "completed", "late", "1600", ""],
    );
    let job = &state.job.jobs["ops"]["job-1"];
    assert_eq!(job.status, JobStatus::Cancelled);
    assert_eq!(job.output, None);
}

#[test]
fn validation_rejects_bad_inputs_and_retry_bounds() {
    let cap = JobQueueCapability;
    let bus = Bus;
    let state = TestState::default();
    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "job.submit",
            &[
                "ops".into(),
                "job-1".into(),
                "__private".into(),
                "[]".into(),
                "".into(),
                "1000".into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("must not start with __"));

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "job.submit",
            &[
                "ops".into(),
                "job-1".into(),
                "work".into(),
                "{}".into(),
                "".into(),
                "1000".into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("JSON array"));

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "job.submit",
            &[
                "ops".into(),
                "job-1".into(),
                "work".into(),
                "[]".into(),
                r#"{"maxAttempts":11}"#.into(),
                "1000".into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("maxAttempts"));
}

#[test]
fn app_removed_drops_jobs() {
    let cap = JobQueueCapability;
    let bus = Bus;
    let mut state = TestState::default();
    submit(&cap, &mut state, &bus, "job-1");
    let removed = encode_event("app.removed", &AppRemoved { id: "ops".into() }).unwrap();
    cap.fold(&mut state, &removed).unwrap();
    assert!(!state.job.jobs.contains_key("ops"));
}
