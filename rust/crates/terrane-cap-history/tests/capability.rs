use std::any::Any;
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::Value;
use terrane_cap_history::{HistoryCapability, HistoryState};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Error, EventRecord, QueryCtx,
    QueryValue, Result, StateStore,
};

#[derive(Default)]
struct TestState {
    history: HistoryState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "history" => Some(&self.history),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "history" => Some(&mut self.history),
            _ => None,
        }
    }
}

struct TestBus {
    apps: BTreeSet<String>,
}

impl TestBus {
    fn new(apps: &[&str]) -> Self {
        Self {
            apps: apps.iter().map(|app| (*app).to_string()).collect(),
        }
    }
}

impl CapBus for TestBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue> {
        match (cap, name, args.first()) {
            ("app", "exists", Some(app)) => Ok(QueryValue::Bool(self.apps.contains(app))),
            _ => Err(Error::InvalidInput(format!("unexpected query {cap}.{name}"))),
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
struct KvSet {
    app: String,
    key: String,
    value: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct KvDeleted {
    app: String,
    key: String,
}

fn kv_set(seq_actor: &str, app: &str, key: &str, value: &str) -> EventRecord {
    let mut record = encode_event(
        "kv.set",
        &KvSet {
            app: app.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        },
    )
    .unwrap();
    record.actor = seq_actor.to_string();
    record
}

fn kv_deleted(seq_actor: &str, app: &str, key: &str) -> EventRecord {
    let mut record = encode_event(
        "kv.deleted",
        &KvDeleted {
            app: app.to_string(),
            key: key.to_string(),
        },
    )
    .unwrap();
    record.actor = seq_actor.to_string();
    record
}

fn fold_all(state: &mut TestState, records: &[EventRecord]) {
    let cap = HistoryCapability;
    for record in records {
        cap.fold(state, record).unwrap();
    }
}

fn query_json(state: &TestState, name: &str, args: &[&str]) -> Value {
    let cap = HistoryCapability;
    let bus = TestBus::new(&["notes"]);
    let args = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    match cap
        .query(
            QueryCtx {
                state,
                bus: &bus,
            },
            name,
            &args,
        )
        .unwrap()
    {
        QueryValue::Json(json) => serde_json::from_str(&json).unwrap(),
        other => panic!("unexpected query value: {other:?}"),
    }
}

#[test]
fn list_key_and_at_cover_kv_history() {
    let records = vec![
        kv_set("user:local-owner", "notes", "title", "one"),
        kv_set("user:local-owner", "notes", "title", "two"),
        kv_deleted("user:local-owner", "notes", "title"),
    ];
    let mut state = TestState::default();
    fold_all(&mut state, &records);

    let list = query_json(&state, "list", &["notes", "kind:kv.set", "", "10"]);
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert_eq!(list["from_seq"], 1);

    let key = query_json(&state, "key", &["notes", "title", "10"]);
    let items = key["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["old"], Value::Null);
    assert_eq!(items[0]["new"], "one");
    assert_eq!(items[1]["old"], "one");
    assert_eq!(items[1]["new"], "two");
    assert_eq!(items[2]["old"], "two");
    assert_eq!(items[2]["new"], Value::Null);

    let at = query_json(&state, "at", &["notes", "title", "1"]);
    assert_eq!(at["value"], "one");
}

#[test]
fn revert_emits_compensating_events_and_replays_identically() {
    let records = vec![
        kv_set("user:local-owner", "notes", "a", "one"),
        kv_set("user:local-owner", "notes", "a", "two"),
        kv_set("user:local-owner", "notes", "b", "new"),
    ];
    let mut state = TestState::default();
    fold_all(&mut state, &records);

    let cap = HistoryCapability;
    let bus = TestBus::new(&["notes"]);
    let args = ["notes", "1", "app", ""]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    let decision = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "history.revert",
            &args,
        )
        .unwrap();
    let Decision::Commit(compensations) = decision else {
        panic!("history.revert must commit");
    };
    assert_eq!(
        compensations
            .iter()
            .filter(|record| record.kind == "kv.set")
            .count(),
        1
    );
    assert_eq!(
        compensations
            .iter()
            .filter(|record| record.kind == "kv.deleted")
            .count(),
        1
    );
    assert_eq!(compensations.last().unwrap().kind, "history.reverted");

    let mut replayed = TestState::default();
    let mut full_log = records;
    full_log.extend(compensations);
    fold_all(&mut replayed, &full_log);
    assert_eq!(
        terrane_cap_history::value_at(&replayed.history, "notes", "a", replayed.history.next_seq)
            .unwrap(),
        Some("one".into())
    );
    assert_eq!(
        terrane_cap_history::value_at(&replayed.history, "notes", "b", replayed.history.next_seq)
            .unwrap(),
        None
    );
}

#[test]
fn actor_filter_uses_record_actor() {
    let records = vec![
        kv_set("user:a", "notes", "a", "one"),
        kv_set("user:b", "notes", "b", "two"),
    ];
    let mut state = TestState::default();
    fold_all(&mut state, &records);

    let list = query_json(&state, "list", &["notes", "actor:user:b", "", "10"]);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["actor"], "user:b");
}

#[test]
fn validation_rejects_bad_scope_and_future_seq() {
    let records = vec![kv_set("user:local-owner", "notes", "a", "one")];
    let mut state = TestState::default();
    fold_all(&mut state, &records);
    let cap = HistoryCapability;
    let bus = TestBus::new(&["notes"]);

    let future = ["notes", "9999", "key", "a"]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "history.revert",
            &future,
        ),
        Err(Error::InvalidInput(
            "to_seq 9999 is beyond current history seq 1".into()
        ))
    );

    let bad_scope = ["notes", "1", "weird", "a"]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &state,
                bus: &bus,
            },
            "history.revert",
            &bad_scope,
        ),
        Err(Error::InvalidInput(
            "history.revert scope must be key, prefix, or app, got weird".into()
        ))
    );
}
