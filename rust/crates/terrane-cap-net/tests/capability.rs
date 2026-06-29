use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};
use terrane_cap_net::{fetched_event, FetchResponse, NetCapability, NetState};

#[derive(Default)]
struct Store {
    net: NetState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "net" => Some(&self.net),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "net" => Some(&mut self.net),
            _ => None,
        }
    }
}

struct AppBus {
    exists: bool,
}

impl CapBus for AppBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.exists)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[derive(BorshSerialize)]
struct Removed {
    id: String,
}

#[test]
fn net_capability_returns_effect_and_folds_recorded_response() {
    let cap = NetCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "net.fetch",
            &["demo".into(), "https://example.test/data".into()],
        )
        .unwrap(),
        Decision::Effect(Effect::HttpGet {
            app: "demo".into(),
            url: "https://example.test/data".into()
        })
    );

    cap.fold(
        &mut store,
        &fetched_event("demo", "https://example.test/data", 204, "".into()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.net.fetches["demo"]["https://example.test/data"],
        FetchResponse {
            status: 204,
            body: String::new()
        }
    );
}

#[test]
fn net_capability_rejects_missing_apps_and_cleans_removed_apps() {
    let cap = NetCapability;
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: false },
            },
            "net.fetch",
            &["demo".into(), "https://example.test".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("demo".into())
    );

    cap.fold(
        &mut store,
        &fetched_event("demo", "https://example.test", 200, "ok".into()).unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(store.net.fetches.is_empty());
}

#[test]
fn net_doc_covers_recorded_http_effects_and_app_cleanup() {
    let doc = NetCapability.doc(false);

    assert_eq!(doc.namespace, "net");
    assert_eq!(doc.manifest.commands, vec!["net.fetch".to_string()]);
    assert_eq!(doc.manifest.events, vec!["net.fetched".to_string()]);
    assert_eq!(doc.manifest.subscriptions, vec!["app.removed".to_string()]);
    assert!(doc.manifest.queries.is_empty());
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Effect")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("never by replay")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Folding app.removed removes")));
    assert!(doc
        .compatibility
        .iter()
        .any(|entry| entry.contains("recording net.fetched")));
    assert!(doc.internal.is_empty());

    assert!(NetCapability
        .doc(true)
        .internal
        .iter()
        .any(|note| note.title.contains("Replay boundary")));
}
