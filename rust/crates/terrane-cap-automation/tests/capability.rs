use terrane_cap_automation::{matching_rules, AutomationCapability, AutomationState};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, EventRecord, QueryValue, StateStore,
};

#[derive(Default)]
struct TestState {
    automation: AutomationState,
    auth: terrane_cap_auth::AuthState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "automation" => Some(&self.automation),
            "auth" => Some(&self.auth),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "automation" => Some(&mut self.automation),
            "auth" => Some(&mut self.auth),
            _ => None,
        }
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

    fn event_kind_matches(&self, pattern: &str) -> bool {
        matches!(pattern, "kv.set" | "kv.*" | "app.removed")
    }
}

fn commit(
    cap: &AutomationCapability,
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
    let cap = AutomationCapability;
    let bus = Bus;
    let mut state = TestState::default();
    let mut log = Vec::new();
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "automation.set",
        &[
            "mailbox",
            "summarize",
            r#"{"trigger":{"kind":"kv.set","filter":"payload.key == 'inbox/1'"},"action":{"verb":"summarize","argsTemplate":["{{payload.key}}"]}}"#,
        ],
    ));
    let hash = state.automation.rules["mailbox"]["summarize"]
        .rule_hash
        .clone();
    log.extend(commit(
        &cap,
        &mut state,
        &bus,
        "automation.fire",
        &["mailbox", "summarize", &hash, "event-1", "2000"],
    ));
    assert_eq!(
        state.automation.rules["mailbox"]["summarize"].fire_count,
        1
    );

    let mut replayed = TestState::default();
    for record in &log {
        cap.fold(&mut replayed, record).unwrap();
    }
    assert_eq!(replayed.automation, state.automation);
}

#[test]
fn invalid_rule_is_rejected() {
    let cap = AutomationCapability;
    let bus = Bus;
    let state = TestState::default();
    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "automation.set",
            &[
                "mailbox".into(),
                "bad".into(),
                r#"{"trigger":{"kind":"ghost.set"},"action":{"verb":"run"}}"#.into(),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("declared event"), "{err}");
}

#[test]
fn matcher_filters_kv_payloads() {
    let cap = AutomationCapability;
    let bus = Bus;
    let mut state = TestState::default();
    commit(
        &cap,
        &mut state,
        &bus,
        "automation.set",
        &[
            "mailbox",
            "summarize",
            r#"{"trigger":{"kind":"kv.set","filter":"payload.key == 'inbox/1'"},"action":{"verb":"summarize","argsTemplate":["{{payload.key}}"]}}"#,
        ],
    );
    let miss = terrane_cap_kv::set_event("mailbox", "archive/1", "old").unwrap();
    assert!(matching_rules(&state.automation, &miss, 2000)
        .unwrap()
        .is_empty());
    let hit = terrane_cap_kv::set_event("mailbox", "inbox/1", "new").unwrap();
    assert_eq!(
        matching_rules(&state.automation, &hit, 2000)
            .unwrap()
            .len(),
        1
    );
}
