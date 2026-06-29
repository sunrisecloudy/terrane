//! Capability-interface contract tests: registry manifest validation and the
//! read-only query bus.

use terrane_cap_app::AppRecord;
use terrane_cap_interface::{
    CapBus, CapManifest, Capability, CommandCtx, CommandSpec, EventPattern, EventSpec, QuerySpec,
    QueryValue, StateStore,
};
use terrane_cap_replica::initialized_event;
use terrane_core::{
    capability_doc, capability_docs, default_registry, fold_records_in_memory, Decision, Error,
    EventRecord, Registry, RegistryBus, Result, State,
};

struct TestCap {
    namespace: &'static str,
    commands: &'static [&'static str],
    events: &'static [&'static str],
    queries: &'static [&'static str],
    subscriptions: &'static [&'static str],
}

impl TestCap {
    fn new(namespace: &'static str) -> Self {
        Self {
            namespace,
            commands: &[],
            events: &[],
            queries: &[],
            subscriptions: &[],
        }
    }

    fn commands(mut self, commands: &'static [&'static str]) -> Self {
        self.commands = commands;
        self
    }

    fn events(mut self, events: &'static [&'static str]) -> Self {
        self.events = events;
        self
    }

    fn queries(mut self, queries: &'static [&'static str]) -> Self {
        self.queries = queries;
        self
    }

    fn subscriptions(mut self, subscriptions: &'static [&'static str]) -> Self {
        self.subscriptions = subscriptions;
        self
    }
}

impl Capability for TestCap {
    fn namespace(&self) -> &'static str {
        self.namespace
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: self
                .commands
                .iter()
                .map(|&name| CommandSpec { name })
                .collect(),
            events: self.events.iter().map(|&kind| EventSpec { kind }).collect(),
            queries: self
                .queries
                .iter()
                .map(|&name| QuerySpec { name })
                .collect(),
            resources: Vec::new(),
            subscriptions: self
                .subscriptions
                .iter()
                .map(|&kind| EventPattern { kind })
                .collect(),
        }
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!("unknown command: {name}")))
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }
}

#[test]
fn default_registry_manifest_is_valid() {
    default_registry().validate().unwrap();
}

#[test]
fn registry_rejects_duplicate_manifest_owners() {
    let mut registry = Registry::new();
    registry
        .try_register(Box::new(
            TestCap::new("dup").commands(&["dup.run", "dup.run"]),
        ))
        .unwrap();
    assert_invalid_contains(registry.validate(), "duplicate command");

    let mut registry = Registry::new();
    registry
        .try_register(Box::new(
            TestCap::new("dup").queries(&["dup.exists", "dup.exists"]),
        ))
        .unwrap();
    assert_invalid_contains(registry.validate(), "duplicate query");

    let mut registry = Registry::new();
    registry
        .try_register(Box::new(
            TestCap::new("dup").events(&["dup.created", "dup.created"]),
        ))
        .unwrap();
    assert_invalid_contains(registry.validate(), "duplicate event");
}

#[test]
fn registry_validates_subscriptions_without_treating_them_as_event_owners() {
    let mut registry = Registry::new();
    registry
        .try_register(Box::new(TestCap::new("app").events(&["app.removed"])))
        .unwrap();
    registry
        .try_register(Box::new(
            TestCap::new("left").subscriptions(&["app.removed"]),
        ))
        .unwrap();
    registry
        .try_register(Box::new(
            TestCap::new("right").subscriptions(&["app.removed"]),
        ))
        .unwrap();
    registry.validate().unwrap();

    let mut registry = Registry::new();
    registry
        .try_register(Box::new(
            TestCap::new("lonely").subscriptions(&["ghost.event"]),
        ))
        .unwrap();
    assert_invalid_contains(registry.validate(), "undeclared event");
}

#[test]
fn registry_bus_exposes_app_exists_and_replica_peer_queries() {
    let registry = default_registry();
    let mut state = State::default();
    state.app.apps.insert(
        "demo".to_string(),
        AppRecord {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            source: None,
        },
    );

    let bus = RegistryBus::new(&registry, &state);
    assert_eq!(
        bus.query("app", "exists", &["demo".to_string()]).unwrap(),
        QueryValue::Bool(true)
    );
    assert_eq!(
        bus.query("app", "exists", &["ghost".to_string()]).unwrap(),
        QueryValue::Bool(false)
    );
    assert_eq!(
        bus.query("replica", "peer", &[]).unwrap(),
        QueryValue::U64(None)
    );

    fold_records_in_memory(&mut state, &[initialized_event(42).unwrap()]).unwrap();
    let bus = RegistryBus::new(&registry, &state);
    assert_eq!(
        bus.query("replica", "peer", &[]).unwrap(),
        QueryValue::U64(Some(42))
    );
}

#[test]
fn registry_bus_reports_unknown_capability_or_query() {
    let registry = default_registry();
    let state = State::default();
    let bus = RegistryBus::new(&registry, &state);

    assert_invalid_contains(
        bus.query("ghost", "exists", &[]),
        "unknown query capability",
    );
    assert_invalid_contains(bus.query("app", "missing", &[]), "unknown query");
}

#[test]
fn capability_docs_include_registered_relational_docs() {
    let docs = capability_docs(false);
    assert!(docs.iter().any(|doc| doc.namespace == "kv"));
    let rdb = docs
        .iter()
        .find(|doc| doc.namespace == "relational_db")
        .expect("relational_db doc");
    assert_eq!(rdb.status, "stable");
    assert!(rdb
        .schemas
        .iter()
        .any(|schema| schema.id == "terrane.relational_db.tableSpec.v1"));
    assert!(rdb
        .schemas
        .iter()
        .any(|schema| schema.id == "terrane.relational_db.query.v1"));
    assert!(rdb.internal.is_empty());

    let internal = capability_doc("relational_db", true).unwrap();
    assert!(internal
        .internal
        .iter()
        .any(|note| note.title.contains("Reserved kv layout")));
}

#[test]
fn capability_doc_unknown_namespace_is_clear() {
    assert_invalid_contains(capability_doc("ghost", false), "unknown command namespace");
}

fn assert_invalid_contains<T: std::fmt::Debug>(result: Result<T>, expected: &str) {
    match result {
        Err(Error::InvalidInput(message)) => {
            assert!(
                message.contains(expected),
                "expected {message:?} to contain {expected:?}"
            );
        }
        other => panic!("expected InvalidInput containing {expected:?}, got {other:?}"),
    }
}
