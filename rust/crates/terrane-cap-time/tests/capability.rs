//! White-box unit coverage for the `time` capability's decide/fold/read surface
//! and internal helpers. The behavioral replay proofs live in the engine e2e
//! tests (`terrane-core/tests/cap/time.rs`, `terrane-host/tests/cap/time.rs`).

use std::any::Any;
use std::time::{Duration, UNIX_EPOCH};

use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, EventRecord, QueryValue,
    ReadValue, ResourceReadCtx, Result, StateStore,
};
use terrane_cap_time::doc::time_doc;
use terrane_cap_time::{
    observed_event, system_time_to_epoch_ms, TimeCapability, TimeState, MAX_OBSERVATIONS_PER_RUN,
};

#[derive(Default)]
struct Store {
    time: TimeState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "time" => Some(&self.time),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "time" => Some(&mut self.time),
            _ => None,
        }
    }
}

struct AppBus {
    exists: bool,
}

impl CapBus for AppBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.exists)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

fn ctx<'a>(store: &'a Store, bus: &'a AppBus) -> CommandCtx<'a> {
    CommandCtx {
        state: store,
        bus,
    }
}

#[test]
fn observed_event_describes_and_folds_last_value() {
    let mut store = Store::default();
    let cap = TimeCapability;
    let record = observed_event("demo", 1_700_000_000_000).unwrap();

    assert_eq!(
        cap.describe(&record).unwrap(),
        "time.observed demo (epoch_ms=1700000000000)"
    );

    cap.fold(&mut store, &record).unwrap();
    assert_eq!(store.time.last["demo"], 1_700_000_000_000);

    // A later observation upserts the last value (no monotonicity guarantee).
    let newer = observed_event("demo", 1_699_999_000_000).unwrap();
    cap.fold(&mut store, &newer).unwrap();
    assert_eq!(store.time.last["demo"], 1_699_999_000_000);
}

#[test]
fn time_now_decides_recorded_effect_for_existing_app() {
    let store = Store::default();
    let bus = AppBus { exists: true };

    assert_eq!(
        TimeCapability
            .decide(ctx(&store, &bus), "time.now", &["demo".into()])
            .unwrap(),
        Decision::Effect(Effect::ObserveTime { app: "demo".into() })
    );
}

#[test]
fn time_live_decides_transient_effect() {
    let store = Store::default();
    let bus = AppBus { exists: true };

    assert_eq!(
        TimeCapability
            .decide(ctx(&store, &bus), "time.live", &["demo".into()])
            .unwrap(),
        Decision::TransientEffect(Effect::ObserveTime { app: "demo".into() })
    );
}

#[test]
fn time_now_rejects_missing_app_before_any_effect() {
    let store = Store::default();
    let bus = AppBus { exists: false };

    assert_eq!(
        TimeCapability
            .decide(ctx(&store, &bus), "time.now", &["ghost".into()])
            .unwrap_err(),
        Error::AppNotFound("ghost".into())
    );
}

#[test]
fn unknown_command_is_a_typed_error() {
    let store = Store::default();
    let bus = AppBus { exists: true };
    assert!(matches!(
        TimeCapability
            .decide(ctx(&store, &bus), "time.when", &["demo".into()])
            .unwrap_err(),
        Error::InvalidInput(_)
    ));
}

#[test]
fn last_returns_last_observation_or_null() {
    let store = Store::default();
    let cap = TimeCapability;
    let read_ctx = ResourceReadCtx {
        state: &store,
        bus: &AppBus { exists: true },
        app: "demo",
        host: None,
    };
    match cap.read_resource(read_ctx, "last", &[]).unwrap() {
        ReadValue::OptString(None) => {}
        other => panic!("expected null before any observation, got {other:?}"),
    }

    let mut store = store;
    let record = observed_event("demo", 1_700_000_000_011).unwrap();
    cap.fold(&mut store, &record).unwrap();
    let read_ctx = ResourceReadCtx {
        state: &store,
        bus: &AppBus { exists: true },
        app: "demo",
        host: None,
    };
    assert_eq!(
        cap.read_resource(read_ctx, "last", &[]).unwrap(),
        ReadValue::OptString(Some("1700000000011".to_string()))
    );
}

#[test]
fn resource_call_output_returns_epoch_ms_string() {
    let store = Store::default();
    let cap = TimeCapability;
    let record = observed_event("demo", 1_700_000_000_042).unwrap();
    for method in &["now", "live"] {
        assert_eq!(
            cap.resource_call_output(&store, "demo", method, std::slice::from_ref(&record))
                .unwrap(),
            ReadValue::OptString(Some("1700000000042".to_string()))
        );
    }
}

#[test]
fn app_removed_drops_the_observation_entry() {
    let mut store = Store::default();
    let cap = TimeCapability;
    cap.fold(&mut store, &observed_event("demo", 42).unwrap())
        .unwrap();
    assert!(store.time.last.contains_key("demo"));

    let removed = make_app_removed("demo");
    cap.fold(&mut store, &removed).unwrap();
    assert!(store.time.last.is_empty());
}

#[test]
fn system_time_to_epoch_ms_handles_now_and_pre_epoch() {
    let post = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    assert_eq!(system_time_to_epoch_ms(post).unwrap(), 1_700_000_000_000);

    let pre = UNIX_EPOCH - Duration::from_secs(1);
    assert!(matches!(
        system_time_to_epoch_ms(pre).unwrap_err(),
        Error::Runtime(_)
    ));
}

#[test]
fn recorded_call_cap_gates_now_only() {
    let cap = TimeCapability;
    let now = cap.recorded_call_per_run_limit("now").unwrap();
    assert_eq!(now.limit, MAX_OBSERVATIONS_PER_RUN);
    assert!(now.escape_hint.contains("time.live"));
    assert!(cap.recorded_call_per_run_limit("live").is_none());
    assert!(cap.recorded_call_per_run_limit("last").is_none());
}

#[test]
fn doc_lists_surface_and_limits() {
    let doc = time_doc(true);
    assert_eq!(doc.namespace, "time");
    assert!(doc.manifest.commands.contains(&"time.now".to_string()));
    assert!(doc.manifest.events.contains(&"time.observed".to_string()));
    assert_eq!(doc.manifest.resource_methods.len(), 3);
    assert!(doc
        .limits
        .iter()
        .any(|l| l.name == "recordedObservationsPerRun"));
    assert!(!doc.internal.is_empty(), "expect internal Date.now warning");
}

/// Build an `app.removed` event matching the shared `AppRemoved` payload shape
/// so the test fixture owns no other crate.
fn make_app_removed(id: &str) -> EventRecord {
    #[derive(borsh::BorshSerialize)]
    struct AppRemoved {
        id: String,
    }
    encode_event("app.removed", &AppRemoved { id: id.to_string() }).unwrap()
}
