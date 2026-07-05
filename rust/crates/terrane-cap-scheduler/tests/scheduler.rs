use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, EventRecord, QueryValue, StateStore,
};

use terrane_cap_scheduler::{schedules_due_at, SchedulerCapability, SchedulerState};

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

#[derive(borsh::BorshSerialize)]
struct AppRemoved {
    id: String,
}

fn commit(
    cap: &SchedulerCapability,
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

#[test]
fn set_fire_and_replay_rebuild_identical_state() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let mut state = TestState::default();
    let mut log = Vec::new();
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.set",
        &[
            "ops",
            "daily",
            r#"{"cron":"*/15 * * * *","verb":"on_timer","args":["daily-digest"]}"#,
        ],
    ));
    assert_eq!(
        state.scheduler.schedules["ops"]["daily"].spec.spec_json,
        r#"{"args":["daily-digest"],"cron":"*/15 * * * *","verb":"on_timer"}"#
    );

    let due = schedules_due_at(&state.scheduler, 3_600_000).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].scheduled_for, 3_600_000);
    assert_eq!(due[0].skipped, 0);

    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.fire",
        &["ops", "daily", "3600000", "3600123", "3"],
    ));
    let entry = &state.scheduler.schedules["ops"]["daily"];
    assert_eq!(entry.last_scheduled_for, Some(3_600_000));
    assert_eq!(entry.last_fired_at, Some(3_600_123));
    assert_eq!(entry.skipped_total, 3);

    let mut replayed = TestState::default();
    for record in &log {
        cap.fold(&mut replayed, record).unwrap();
    }
    assert_eq!(replayed.scheduler, state.scheduler);
}

#[test]
fn one_shot_due_once_and_is_dropped_after_fire() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let mut state = TestState::default();
    commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.set",
        &["ops", "once", r#"{"at":1000}"#],
    );
    assert_eq!(schedules_due_at(&state.scheduler, 999).unwrap(), Vec::new());
    assert_eq!(schedules_due_at(&state.scheduler, 1000).unwrap().len(), 1);
    commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.fire",
        &["ops", "once", "1000", "2000", "0"],
    );
    assert!(!state.scheduler.schedules["ops"].contains_key("once"));
}

#[test]
fn catch_up_collapses_missed_cron_occurrences() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let mut state = TestState::default();
    commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.set",
        &["ops", "quarter", r#"{"cron":"*/15 * * * *"}"#],
    );
    commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.fire",
        &["ops", "quarter", "900000", "900100", "0"],
    );
    let due = schedules_due_at(&state.scheduler, 3_700_000).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].scheduled_for, 3_600_000);
    assert_eq!(due[0].skipped, 2);
}

#[test]
fn invalid_spec_and_limits_are_rejected() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let state = TestState::default();
    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "scheduler.set",
            &[
                "ops".into(),
                "bad".into(),
                r#"{"cron":"* * * * *","at":1}"#.into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("exactly one of at or cron"));

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "scheduler.set",
            &[
                "ops".into(),
                "bad".into(),
                r#"{"cron":"* * * * *","verb":"__private"}"#.into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("must not start with __"));
}

#[test]
fn app_removed_drops_schedules() {
    let cap = SchedulerCapability;
    let bus = Bus;
    let mut state = TestState::default();
    commit(
        &cap,
        &mut state,
        &bus,
        "scheduler.set",
        &["ops", "daily", r#"{"at":1000}"#],
    );
    let removed = encode_event(
        "app.removed",
        &AppRemoved {
            id: "ops".to_string(),
        },
    )
    .unwrap();
    cap.fold(&mut state, &removed).unwrap();
    assert!(!state.scheduler.schedules.contains_key("ops"));
}
