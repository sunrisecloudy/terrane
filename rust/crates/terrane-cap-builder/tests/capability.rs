use std::any::Any;

use terrane_cap_builder::{
    failed_event, generated_event, parse_generated_files, requested_event, BuilderCapability,
    BuilderFile, BuilderState,
};
use terrane_cap_interface::{CapBus, Capability, CommandCtx, Error, QueryValue, StateStore};

const ALLOWED_RESOURCES: &[&str] = &["kv", "crdt", "relational_db", "build"];

#[derive(Default)]
struct Store {
    builder: BuilderState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "builder" => Some(&self.builder),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "builder" => Some(&mut self.builder),
            _ => None,
        }
    }
}

struct NoBus;

impl CapBus for NoBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

fn generated_json() -> String {
    r#"{"files":[
{"path":"manifest.json","content":"{\"id\":\"calendar\",\"name\":\"Calendar\",\"runtime\":\"js\",\"backend\":\"main.js\",\"ui\":\"index.html\",\"resources\":[\"kv\",\"crdt\"]}"},
{"path":"main.js","content":"export function run(){ return 1; }"},
{"path":"index.html","content":"<!doctype html><title>Calendar</title>"},
{"path":"style.css","content":"body { color: black; }"}
]}"#
    .to_string()
}

#[test]
fn builder_capability_validates_generated_bundle_and_folds_lifecycle() {
    let files = parse_generated_files(&generated_json(), "calendar", "Calendar", ALLOWED_RESOURCES)
        .unwrap();
    assert_eq!(files.len(), 4);
    assert_eq!(files[0].path, "index.html");

    let cap = BuilderCapability;
    let mut store = Store::default();
    cap.fold(
        &mut store,
        &requested_event("draft-1", "calendar", "Calendar", "make calendar", "codex").unwrap(),
    )
    .unwrap();
    cap.fold(&mut store, &generated_event("draft-1", files).unwrap())
        .unwrap();

    let draft = &store.builder.drafts["draft-1"];
    assert_eq!(draft.app_id, "calendar");
    assert_eq!(draft.files.len(), 4);
    assert_eq!(draft.error, None);
}

#[test]
fn builder_capability_rejects_commands_and_records_failures() {
    let cap = BuilderCapability;
    let bus = NoBus;
    let mut store = Store::default();

    assert!(cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "builder.generate",
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("unknown command"));

    cap.fold(&mut store, &failed_event("draft-1", "bad schema").unwrap())
        .unwrap();
    assert_eq!(
        store.builder.drafts["draft-1"].error.as_deref(),
        Some("bad schema")
    );
    assert!(parse_generated_files(
        r#"{"files":[{"path":"manifest.json","content":"{}"}]}"#,
        "calendar",
        "Calendar",
        ALLOWED_RESOURCES,
    )
    .is_err());
}

#[test]
fn builder_generated_event_can_create_a_draft_from_files_only() {
    let cap = BuilderCapability;
    let mut store = Store::default();

    cap.fold(
        &mut store,
        &generated_event(
            "draft-2",
            vec![BuilderFile {
                path: "manifest.json".into(),
                content: "{}".into(),
            }],
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(store.builder.drafts["draft-2"].id, "draft-2");
    assert_eq!(store.builder.drafts["draft-2"].files.len(), 1);
}
