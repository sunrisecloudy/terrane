use std::any::Any;

use terrane_cap_app::{AppCapability, AppState};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Error, QueryCtx, QueryValue, StateStore,
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
