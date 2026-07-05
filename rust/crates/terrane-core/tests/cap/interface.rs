//! Capability-interface contract tests: registry manifest validation and the
//! read-only query bus.

use terrane_cap_app::AppRecord;
use terrane_cap_interface::{
    CapBus, CapManifest, Capability, CapabilityDoc, CapabilityManifestDoc, CommandCtx, CommandSpec,
    EventPattern, EventSpec, GrantResourceSpec, QuerySpec, QueryValue, ResourceMethod, StateStore,
};
use terrane_cap_replica::initialized_event;
use terrane_core::{
    capability_doc, capability_docs, default_registry, fold_records_in_memory,
    grant_resource_namespaces, Decision, Error, EventRecord, Registry, RegistryBus, Result, State,
};

struct TestCap {
    namespace: &'static str,
    commands: &'static [&'static str],
    events: &'static [&'static str],
    queries: &'static [&'static str],
    resource_methods: &'static [&'static str],
    grant_namespace_v1: bool,
    subscriptions: &'static [&'static str],
}

impl TestCap {
    fn new(namespace: &'static str) -> Self {
        Self {
            namespace,
            commands: &[],
            events: &[],
            queries: &[],
            resource_methods: &[],
            grant_namespace_v1: false,
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

    fn resources(mut self, resource_methods: &'static [&'static str]) -> Self {
        self.resource_methods = resource_methods;
        self
    }

    fn grant_namespace_v1(mut self) -> Self {
        self.grant_namespace_v1 = true;
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
            resources: self
                .resource_methods
                .iter()
                .map(|&name| ResourceMethod::Read { name, params: &[] })
                .collect(),
            grant_resources: if self.grant_namespace_v1 {
                vec![GrantResourceSpec::namespace_v1(
                    self.namespace,
                    &["read"],
                    "Test resource.",
                )]
            } else {
                Vec::new()
            },
            subscriptions: self
                .subscriptions
                .iter()
                .map(|&kind| EventPattern { kind })
                .collect(),
        }
    }

    fn doc(&self, _include_internal: bool) -> CapabilityDoc {
        CapabilityDoc {
            namespace: self.namespace.to_string(),
            title: self.namespace.to_string(),
            summary: format!("Test capability `{}`.", self.namespace),
            status: "test".to_string(),
            version: "0.0.0".to_string(),
            audience: vec!["test".to_string()],
            manifest: CapabilityManifestDoc {
                commands: self
                    .commands
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                queries: self
                    .queries
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                events: self.events.iter().map(|name| (*name).to_string()).collect(),
                subscriptions: self
                    .subscriptions
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                resource_methods: Vec::new(),
            },
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            resources: Vec::new(),
            schemas: Vec::new(),
            examples: Vec::new(),
            constraints: Vec::new(),
            limits: Vec::new(),
            compatibility: Vec::new(),
            internal: Vec::new(),
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
fn default_registry_exposes_registered_grant_resource_namespaces() {
    assert_eq!(
        grant_resource_namespaces(),
        vec![
            "blob",
            "browser",
            "build",
            "connection",
            "crdt",
            "crypto",
            "document",
            "history",
            "interop",
            "kv",
            "local-model",
            "media",
            "native",
            "net",
            "query",
            "relational_db",
            "scheduler",
            "search",
            "stt",
            "sysinfo",
            "telemetry",
            "time"
        ]
    );
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
fn registry_rejects_resource_methods_without_grant_resource_specs() {
    let mut registry = Registry::new();
    registry
        .try_register(Box::new(TestCap::new("files").resources(&["get"])))
        .unwrap();
    assert_invalid_contains(registry.validate(), "without grant resource specs");

    let mut registry = Registry::new();
    registry
        .try_register(Box::new(
            TestCap::new("files")
                .resources(&["get"])
                .grant_namespace_v1(),
        ))
        .unwrap();
    registry.validate().unwrap();
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
            runtime: "js".to_string(),
            interfaces: terrane_cap_app::mandatory_interfaces(),
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
fn capability_docs_include_registered_capability_docs() {
    let docs = capability_docs(false);
    assert!(docs.iter().any(|doc| doc.namespace == "kv"));
    let document = docs
        .iter()
        .find(|doc| doc.namespace == "document")
        .expect("document doc");
    assert_eq!(document.status, "stable");
    assert!(document
        .schemas
        .iter()
        .any(|schema| schema.id == "document.schema.json"));
    assert!(document
        .constraints
        .iter()
        .any(|constraint| constraint.contains("RFC 7386 JSON merge-patch")));
    let document_create_command = document
        .commands
        .iter()
        .find(|command| command.name == "document.create")
        .expect("document create command doc");
    assert!(document_create_command
        .emits
        .iter()
        .any(|event| event == "document.created"));
    assert!(document_create_command
        .errors
        .iter()
        .any(|error| error.contains("document quota exceeded")));
    assert!(document
        .events
        .iter()
        .any(|event| event.kind == "document.deleted"));
    let document_create = document
        .manifest
        .resource_methods
        .iter()
        .find(|method| method.name == "create")
        .expect("document create method");
    assert_eq!(document_create.returns, "void");
    assert!(document_create
        .errors
        .iter()
        .any(|error| error.contains("document quota exceeded")));
    let document_get = document
        .manifest
        .resource_methods
        .iter()
        .find(|method| method.name == "get")
        .expect("document get method");
    assert_eq!(document_get.returns, "string|null");
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

    let document_internal = capability_doc("document", true).unwrap();
    assert!(document_internal
        .internal
        .iter()
        .any(|note| note.title.contains("Persistence")));
}

/// Every method a capability installs on `ctx.resource` must be documented
/// in its CapabilityDoc — an app author (or a blind agent reading
/// capability_info) discovers the in-backend surface only through these docs.
/// Catches the gap where a manifest declares a resource method but
/// `resource_methods`/`resources` stay empty.
#[test]
fn every_declared_resource_method_is_documented() {
    let declared = terrane_core::declared_resource_surface();
    assert!(!declared.is_empty());
    let docs = capability_docs(true);
    for entry in &declared {
        let rest = entry
            .strip_prefix("ctx.resource.")
            .unwrap_or_else(|| panic!("unexpected surface entry {entry}"));
        let (namespace, method) = rest
            .rsplit_once('.')
            .unwrap_or_else(|| panic!("unexpected surface entry {entry}"));
        let doc = docs
            .iter()
            .find(|doc| doc.namespace == namespace)
            .unwrap_or_else(|| panic!("no capability doc for resource namespace {namespace}"));
        let documented = doc
            .manifest
            .resource_methods
            .iter()
            .find(|m| m.name == method)
            .unwrap_or_else(|| {
                panic!("{namespace} missing resource method doc for {method} ({entry})")
            });
        assert!(
            !documented.summary.trim().is_empty(),
            "{entry} needs a summary"
        );
        assert!(
            doc.resources
                .iter()
                .any(|resource| resource.namespace == namespace),
            "{namespace} declares resource methods but has no ResourceDoc"
        );
    }
}

#[test]
fn all_capability_docs_are_explicit_and_operational() {
    for doc in capability_docs(true) {
        assert!(
            !doc.summary.contains("Capability namespace `"),
            "{} still looks like a generated fallback summary",
            doc.namespace
        );
        assert!(
            !doc.internal
                .iter()
                .any(|note| note.title.contains("Generated from manifest")),
            "{} still exposes fallback internal notes",
            doc.namespace
        );

        for command_name in &doc.manifest.commands {
            let command = doc
                .commands
                .iter()
                .find(|command| &command.name == command_name)
                .unwrap_or_else(|| {
                    panic!("{} missing command doc for {command_name}", doc.namespace)
                });
            assert!(!command.summary.trim().is_empty(), "{command_name} summary");
            assert!(!command.returns.trim().is_empty(), "{command_name} returns");
            assert!(!command.errors.is_empty(), "{command_name} errors");
            for param in &command.params {
                assert!(
                    !param.summary.trim().is_empty(),
                    "{command_name}.{} summary",
                    param.name
                );
            }
        }

        for query_name in &doc.manifest.queries {
            let query = doc
                .queries
                .iter()
                .find(|query| &query.name == query_name)
                .unwrap_or_else(|| panic!("{} missing query doc for {query_name}", doc.namespace));
            assert!(!query.summary.trim().is_empty(), "{query_name} summary");
            assert!(!query.returns.trim().is_empty(), "{query_name} returns");
            assert!(!query.errors.is_empty(), "{query_name} errors");
            for param in &query.params {
                assert!(
                    !param.summary.trim().is_empty(),
                    "{query_name}.{} summary",
                    param.name
                );
            }
        }

        for event_kind in &doc.manifest.events {
            let event = doc
                .events
                .iter()
                .find(|event| &event.kind == event_kind)
                .unwrap_or_else(|| panic!("{} missing event doc for {event_kind}", doc.namespace));
            assert!(!event.summary.trim().is_empty(), "{event_kind} summary");
            assert!(
                !event.params.is_empty() || !event.examples.is_empty(),
                "{event_kind} needs payload docs or examples"
            );
            for param in &event.params {
                assert!(
                    !param.summary.trim().is_empty(),
                    "{event_kind}.{} summary",
                    param.name
                );
            }
        }

        if !doc.resources.is_empty() {
            assert!(
                !doc.examples.is_empty(),
                "{} resource docs need at least one app-building example",
                doc.namespace
            );
        }
        for resource in &doc.resources {
            assert!(
                !resource.summary.trim().is_empty(),
                "{} resource summary",
                doc.namespace
            );
            for method in &resource.methods {
                assert!(
                    !method.summary.trim().is_empty(),
                    "{}.{} summary",
                    resource.namespace,
                    method.name
                );
                assert!(
                    !method.returns.trim().is_empty(),
                    "{}.{} returns",
                    resource.namespace,
                    method.name
                );
                assert!(
                    !method.errors.is_empty(),
                    "{}.{} errors",
                    resource.namespace,
                    method.name
                );
                for param in &method.params {
                    assert!(
                        !param.summary.trim().is_empty(),
                        "{}.{}({}) summary",
                        resource.namespace,
                        method.name,
                        param.name
                    );
                }
            }
        }
    }
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
