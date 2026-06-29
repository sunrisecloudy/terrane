use std::any::Any;

use borsh::{BorshDeserialize, BorshSerialize};

use super::*;

#[derive(Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct SampleEvent {
    value: String,
}

#[derive(Default)]
struct SampleState {
    value: String,
}

#[derive(Default)]
struct Store {
    sample: SampleState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "sample" => Some(&self.sample),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "sample" => Some(&mut self.sample),
            _ => None,
        }
    }
}

struct Bus {
    app_exists: QueryValue,
    replica_peer: QueryValue,
}

impl CapBus for Bus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(self.app_exists.clone()),
            ("replica", "peer") => Ok(self.replica_peer.clone()),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn event_encoding_round_trips_payloads() {
    let record = encode_event(
        "sample.changed",
        &SampleEvent {
            value: "hello".into(),
        },
    )
    .unwrap();

    assert_eq!(record.kind, "sample.changed");
    assert_eq!(
        decode_event::<SampleEvent>(&record).unwrap(),
        SampleEvent {
            value: "hello".into()
        }
    );
}

#[test]
fn namespace_and_argument_helpers_return_clear_errors() {
    assert_eq!(namespace_of("kv.set").unwrap(), "kv");
    assert!(namespace_of("bad")
        .unwrap_err()
        .to_string()
        .contains("namespace"));
    assert_eq!(arg(&["a".into()], 0, "value").unwrap(), "a");
    assert!(arg(&[], 0, "value")
        .unwrap_err()
        .to_string()
        .contains("missing value"));
}

#[test]
fn state_helpers_downcast_typed_slices() {
    let mut store = Store::default();
    state_mut::<SampleState>(&mut store, "sample")
        .unwrap()
        .value = "stored".into();

    assert_eq!(
        state_ref::<SampleState>(&store, "sample").unwrap().value,
        "stored"
    );
    assert!(state_ref::<SampleState>(&store, "missing").is_err());
}

#[test]
fn bus_helpers_validate_expected_query_types() {
    let bus = Bus {
        app_exists: QueryValue::Bool(true),
        replica_peer: QueryValue::U64(Some(42)),
    };
    assert!(app_exists(&bus, "demo").unwrap());
    assert!(ensure_app_exists(&bus, "demo").is_ok());
    assert_eq!(replica_peer(&bus).unwrap(), Some(42));

    let bad = Bus {
        app_exists: QueryValue::U64(None),
        replica_peer: QueryValue::Bool(true),
    };
    assert!(app_exists(&bad, "demo")
        .unwrap_err()
        .to_string()
        .contains("unexpected"));
    assert!(replica_peer(&bad)
        .unwrap_err()
        .to_string()
        .contains("unexpected"));
}

#[test]
fn capability_doc_is_required_and_structured() {
    struct DocCap;

    impl Capability for DocCap {
        fn namespace(&self) -> &'static str {
            "doc"
        }

        fn manifest(&self) -> CapManifest {
            CapManifest {
                commands: vec![CommandSpec { name: "doc.run" }],
                events: vec![EventSpec { kind: "doc.ran" }],
                queries: vec![QuerySpec { name: "doc.exists" }],
                resources: vec![ResourceMethod::Read {
                    name: "get",
                    params: &["key"],
                }],
                subscriptions: vec![EventPattern {
                    kind: "app.removed",
                }],
            }
        }

        fn doc(&self, include_internal: bool) -> CapabilityDoc {
            let command_example = ExampleDoc {
                title: "Run doc command".into(),
                summary: "Records a doc.ran event.".into(),
                language: "text".into(),
                code: "doc.run key".into(),
                expected: "doc.ran".into(),
            };
            let query_example = ExampleDoc {
                title: "Check doc existence".into(),
                summary: "Returns whether a key exists.".into(),
                language: "text".into(),
                code: "doc.exists key".into(),
                expected: "true".into(),
            };
            let event_example = ExampleDoc {
                title: "Doc ran payload".into(),
                summary: "The event stores the affected key.".into(),
                language: "json".into(),
                code: r#"{"key":"demo"}"#.into(),
                expected: "folded state changes".into(),
            };
            CapabilityDoc {
                namespace: "doc".into(),
                title: "Doc".into(),
                summary: "Structured docs for the doc test capability.".into(),
                status: "stable".into(),
                version: "0.1.0".into(),
                audience: vec!["agent".into()],
                manifest: CapabilityManifestDoc {
                    commands: vec!["doc.run".into()],
                    events: vec!["doc.ran".into()],
                    queries: vec!["doc.exists".into()],
                    subscriptions: vec!["app.removed".into()],
                    resource_methods: vec![resource_method(
                        "get",
                        "read",
                        &[param("key", "Lookup key.", "doc.key")],
                        "Read one doc entry.",
                    )],
                },
                commands: vec![command_doc(
                    "doc.run",
                    &[param("key", "Lookup key.", "doc.key")],
                    "Decision",
                    "Record one doc command.",
                )
                .with_errors(&["invalid input"])
                .with_emits(&["doc.ran"])
                .with_effects(&["updates doc state"])
                .with_examples(&[command_example])],
                queries: vec![query_doc(
                    "doc.exists",
                    &[param("key", "Lookup key.", "doc.key")],
                    "bool",
                    "Check whether a doc entry exists.",
                )
                .with_errors(&["missing key"])
                .with_examples(&[query_example])],
                events: vec![event_doc(
                    "doc.ran",
                    &[param("key", "Lookup key.", "doc.key")],
                    "A doc command completed.",
                )
                .with_effects(&["folds into doc state"])
                .with_examples(&[event_example])],
                resources: vec![ResourceDoc {
                    namespace: "doc".into(),
                    summary: "Backend resource surface for doc.".into(),
                    methods: vec![resource_method(
                        "get",
                        "read",
                        &[param("key", "Lookup key.", "doc.key")],
                        "Read one doc entry.",
                    )],
                }],
                schemas: Vec::new(),
                examples: Vec::new(),
                constraints: Vec::new(),
                limits: Vec::new(),
                compatibility: Vec::new(),
                internal: if include_internal {
                    vec![InternalNote {
                        title: "Fixture".into(),
                        body: "Internal fixture note.".into(),
                    }]
                } else {
                    Vec::new()
                },
            }
        }

        fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
            Err(Error::InvalidInput(format!("unknown command: {name}")))
        }

        fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
            Ok(())
        }
    }

    let public = DocCap.doc(false);
    assert_eq!(public.namespace, "doc");
    assert_eq!(public.manifest.commands, vec!["doc.run"]);
    assert_eq!(public.manifest.events, vec!["doc.ran"]);
    assert_eq!(public.manifest.queries, vec!["doc.exists"]);
    assert_eq!(public.manifest.subscriptions, vec!["app.removed"]);
    assert_eq!(public.commands[0].name, "doc.run");
    assert_eq!(public.commands[0].returns, "Decision");
    assert_eq!(public.commands[0].emits, vec!["doc.ran"]);
    assert_eq!(public.commands[0].effects, vec!["updates doc state"]);
    assert_eq!(public.commands[0].examples[0].title, "Run doc command");
    assert_eq!(public.queries[0].name, "doc.exists");
    assert_eq!(public.queries[0].returns, "bool");
    assert_eq!(public.queries[0].errors, vec!["missing key"]);
    assert_eq!(public.events[0].kind, "doc.ran");
    assert_eq!(public.events[0].params[0].schema_ref, "doc.key");
    assert_eq!(public.resources[0].methods[0].name, "get");
    assert_eq!(public.resources[0].methods[0].kind, "read");
    assert_eq!(public.resources[0].methods[0].params[0].name, "key");
    assert!(public.internal.is_empty());

    let internal = DocCap.doc(true);
    assert_eq!(internal.internal.len(), 1);
}

#[test]
fn json_extraction_and_truncation_are_deterministic() {
    assert_eq!(
        extract_json_object("prefix {\"ok\":true} suffix", "sample").unwrap(),
        "{\"ok\":true}"
    );
    assert!(extract_json_object("no json", "sample").is_err());
    assert_eq!(truncate("abcdef", 3), "abc...");
    assert_eq!(truncate("abc", 3), "abc");
}
