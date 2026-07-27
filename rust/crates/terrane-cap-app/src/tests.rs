use std::any::Any;

use super::*;

#[derive(Default)]
struct Store {
    app: AppState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "app" => Some(&self.app),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "app" => Some(&mut self.app),
            _ => None,
        }
    }
}

struct NoBus;

impl terrane_cap_interface::CapBus for NoBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

#[test]
fn parse_add_collects_multi_word_name_and_optional_source() {
    let args = vec![
        "demo".into(),
        "Daily".into(),
        "Calendar".into(),
        "--source".into(),
        "/tmp/demo".into(),
    ];

    assert_eq!(
        parse_add(&args).unwrap(),
        (
            "demo".into(),
            "Daily Calendar".into(),
            Some("/tmp/demo".into()),
            "js".into(),
            mandatory_interfaces(),
            Vec::new(),
            false
        )
    );
}

#[test]
fn parse_add_accepts_explicit_runtime() {
    let args = vec![
        "demo".into(),
        "Daily".into(),
        "--runtime".into(),
        "wasm".into(),
        "--source".into(),
        "/tmp/demo".into(),
    ];

    assert_eq!(
        parse_add(&args).unwrap(),
        (
            "demo".into(),
            "Daily".into(),
            Some("/tmp/demo".into()),
            "wasm".into(),
            mandatory_interfaces(),
            Vec::new(),
            false
        )
    );
}

#[test]
fn parse_add_requires_a_name_and_source_value() {
    assert!(parse_add(&["demo".into()])
        .unwrap_err()
        .to_string()
        .contains("usage"));
    assert!(
        parse_add(&["demo".into(), "Demo".into(), "--source".into()])
            .unwrap_err()
            .to_string()
            .contains("needs a path")
    );
}

#[test]
fn add_event_describes_and_folds_into_state() {
    let cap = AppCapability;
    let bus = NoBus;
    let mut store = Store::default();
    let args = vec!["demo".into(), "Demo".into()];
    let decision = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.add",
            &args,
        )
        .unwrap();
    let Decision::Commit(events) = decision else {
        panic!("expected app.add to commit");
    };

    assert_eq!(
        cap.describe(&events[0]).unwrap(),
        "app.added demo \"Demo\" runtime=js"
    );
    cap.fold(&mut store, &events[0]).unwrap();
    for event in &events[1..] {
        cap.fold(&mut store, event).unwrap();
    }
    assert_eq!(store.app.apps["demo"].name, "Demo");
    assert_eq!(store.app.apps["demo"].runtime, "js");
    assert_eq!(store.app.apps["demo"].links.len(), 4);
    assert!(store.app.apps["demo"]
        .links
        .iter()
        .any(|link| link.spec == "terrane://app/demo/route/*"));
    assert!(!store.app.apps["demo"].requirements_resolved);
}

#[test]
fn required_capabilities_are_persisted_and_normalized() {
    let cap = AppCapability;
    let mut store = Store::default();
    let event = added_event_with_requirements(
        "demo",
        "Demo",
        None,
        "js",
        mandatory_interfaces(),
        Some(vec!["net".into(), "kv".into(), "net".into()]),
    )
    .unwrap();
    cap.fold(&mut store, &event).unwrap();
    assert_eq!(
        store.app.apps["demo"].required_capabilities,
        vec!["kv", "net"]
    );
    assert!(store.app.apps["demo"].requirements_resolved);
}

#[test]
fn legacy_snapshot_restores_as_unresolved_without_data_loss() {
    let cap = AppCapability;
    let mut legacy = LegacyAppState {
        apps: BTreeMap::new(),
        links: BTreeMap::new(),
    };
    legacy.apps.insert(
        "demo".into(),
        LegacyAppRecord {
            id: "demo".into(),
            name: "Demo".into(),
            source: Some("/tmp/demo".into()),
            runtime: "js".into(),
            version: DEFAULT_VERSION.into(),
            history: Vec::new(),
            interfaces: mandatory_interfaces(),
            links: Vec::new(),
        },
    );
    let mut store = Store::default();
    cap.restore(&mut store, &borsh::to_vec(&legacy).unwrap())
        .unwrap();

    assert_eq!(store.app.apps["demo"].source.as_deref(), Some("/tmp/demo"));
    assert!(!store.app.apps["demo"].requirements_resolved);
}
