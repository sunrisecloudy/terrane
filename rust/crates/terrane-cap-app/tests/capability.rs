use std::any::Any;

use terrane_cap_app::{AppCapability, AppState};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryCtx, QueryValue, StateStore,
};

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

#[test]
fn app_capability_adds_queries_and_removes_apps() {
    let cap = AppCapability;
    let bus = NoBus;
    let mut store = Store::default();

    let Decision::Commit(add_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.add",
            &[
                "calendar".into(),
                "Calendar".into(),
                "--source".into(),
                "/tmp/calendar".into(),
            ],
        )
        .unwrap()
    else {
        panic!("app.add should commit");
    };
    assert_eq!(add_events[0].kind, "app.added");
    cap.fold(&mut store, &add_events[0]).unwrap();

    assert_eq!(
        store.app.apps["calendar"].source.as_deref(),
        Some("/tmp/calendar")
    );
    assert_eq!(
        cap.query(
            QueryCtx {
                state: &store,
                bus: &bus,
            },
            "exists",
            &["calendar".into()],
        )
        .unwrap(),
        QueryValue::Bool(true)
    );

    let Decision::Commit(remove_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.remove",
            &["calendar".into()],
        )
        .unwrap()
    else {
        panic!("app.remove should commit");
    };
    cap.fold(&mut store, &remove_events[0]).unwrap();
    assert!(store.app.apps.is_empty());
}

#[test]
fn app_capability_rejects_duplicate_and_missing_removes() {
    let cap = AppCapability;
    let bus = NoBus;
    let mut store = Store::default();

    let Decision::Commit(events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.add",
            &["demo".into(), "Demo".into()],
        )
        .unwrap()
    else {
        panic!("app.add should commit");
    };
    cap.fold(&mut store, &events[0]).unwrap();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.add",
            &["demo".into(), "Demo".into()],
        )
        .unwrap_err(),
        Error::AppExists("demo".into())
    );
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.remove",
            &["missing".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("missing".into())
    );
}

#[test]
fn app_import_is_effectful() {
    let cap = AppCapability;
    let bus = NoBus;
    let store = Store::default();

    let Decision::Effect(Effect::ImportAppBundle {
        source,
        storage_backend,
        storage_path,
    }) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "app.import",
            &[
                "/tmp/calendar".into(),
                "--storage".into(),
                "sqlite".into(),
                "--path".into(),
                "apps/calendar.sqlite3".into(),
            ],
        )
        .unwrap()
    else {
        panic!("app.import should request an import effect");
    };
    assert_eq!(source, "/tmp/calendar");
    assert_eq!(storage_backend.as_deref(), Some("sqlite"));
    assert_eq!(storage_path.as_deref(), Some("apps/calendar.sqlite3"));
}

#[test]
fn app_doc_covers_manifest_and_removal_cleanup_boundary() {
    let doc = AppCapability.doc(false);

    assert_eq!(doc.namespace, "app");
    assert_eq!(
        doc.manifest.commands,
        vec![
            "app.add".to_string(),
            "app.import".to_string(),
            "app.remove".to_string()
        ]
    );
    assert_eq!(doc.manifest.queries, vec!["app.exists".to_string()]);
    assert_eq!(
        doc.manifest.events,
        vec!["app.added".to_string(), "app.removed".to_string()]
    );
    assert!(doc.manifest.resource_methods.is_empty());
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Replay rebuilds the catalog")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("app.removed")));
    assert!(doc
        .compatibility
        .iter()
        .any(|entry| entry.contains("cleanup boundary")));
    assert!(doc.internal.is_empty());

    assert!(AppCapability
        .doc(true)
        .internal
        .iter()
        .any(|note| note.title.contains("Removal boundary")));
}
