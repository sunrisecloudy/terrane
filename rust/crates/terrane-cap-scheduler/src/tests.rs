use terrane_cap_interface::{CapBus, Capability, CommandCtx, Decision, QueryValue, StateStore};

use crate::{SchedulerCapability, SchedulerState};

#[derive(Default)]
struct TestState {
    scheduler: SchedulerState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        (namespace == "scheduler").then_some(&self.scheduler as &dyn std::any::Any)
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        (namespace == "scheduler").then_some(&mut self.scheduler as &mut dyn std::any::Any)
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        let _ = args;
        if cap == "app" && name == "exists" {
            Ok(QueryValue::Bool(true))
        } else {
            Ok(QueryValue::Bool(false))
        }
    }
}

#[test]
fn create_start_complete_replays_schedule_state() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let mut state = TestState::default();
    let ctx = CommandCtx {
        state: &state,
        bus: &bus,
    };
    let records = match cap
        .decide(
            ctx,
            "scheduler.create",
            &[
                "ops".into(),
                "quickjs-ops-heartbeat".into(),
                "* * * * *".into(),
                "Asia/Bangkok".into(),
                "opsHeartbeat".into(),
                r#"{"source":"premium-ops-proof"}"#.into(),
            ],
        )
        .unwrap()
    {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    for record in &records {
        cap.fold(&mut state, record).unwrap();
    }
    assert_eq!(
        state.scheduler.schedules["ops"]["quickjs-ops-heartbeat"].next_due_at,
        0
    );

    let ctx = CommandCtx {
        state: &state,
        bus: &bus,
    };
    let records = match cap
        .decide(
            ctx,
            "scheduler.run.start",
            &[
                "ops".into(),
                "quickjs-ops-heartbeat".into(),
                "run-1".into(),
                "60".into(),
            ],
        )
        .unwrap()
    {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    for record in &records {
        cap.fold(&mut state, record).unwrap();
    }
    assert_eq!(
        state.scheduler.schedules["ops"]["quickjs-ops-heartbeat"].active_run_id,
        Some("run-1".into())
    );

    let ctx = CommandCtx {
        state: &state,
        bus: &bus,
    };
    let records = match cap
        .decide(
            ctx,
            "scheduler.run.complete",
            &[
                "ops".into(),
                "quickjs-ops-heartbeat".into(),
                "run-1".into(),
                "61".into(),
                r#"{"ok":true}"#.into(),
            ],
        )
        .unwrap()
    {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    for record in &records {
        cap.fold(&mut state, record).unwrap();
    }
    let schedule = &state.scheduler.schedules["ops"]["quickjs-ops-heartbeat"];
    assert_eq!(schedule.active_run_id, None);
    assert_eq!(schedule.next_due_at, 120);
    assert_eq!(
        state.scheduler.runs["ops"]["run-1"].status.as_str(),
        "completed"
    );
}

#[test]
fn invalid_cron_is_rejected() {
    let cap = SchedulerCapability;
    let state = TestState::default();
    let bus = Bus;
    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "scheduler.create",
            &[
                "ops".into(),
                "bad".into(),
                "not cron".into(),
                "Asia/Bangkok".into(),
                "opsHeartbeat".into(),
                "{}".into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("cron must have five fields"));
}
