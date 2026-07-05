use std::any::Any;
use std::collections::BTreeMap;

use borsh::BorshSerialize;
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};
use terrane_cap_net::request::{prepare_request, REDACTED};
use terrane_cap_net::{
    fetched_event, responded_event, FetchResponse, NetCapability, NetState, RecordedBody,
};

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
fn net_request_canonicalizes_redacts_and_folds_recorded_response() {
    let cap = NetCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();
    let raw = r#"{
        "url":"https://example.test/items?token=query",
        "method":"post",
        "headers":{
            "Authorization":"Bearer raw-secret",
            "X-Trace":"abc",
            "X-Internal-Auth":"hidden"
        },
        "sensitiveHeaders":["x-internal-auth"],
        "body":"{\"ok\":true}",
        "timeoutMs":1000,
        "redirect":"manual",
        "responseBody":"inline"
    }"#;
    let prepared = prepare_request(raw).unwrap();

    assert_eq!(prepared.method, "POST");
    assert!(prepared.canonical_json.contains("\"authorization\""));
    assert!(prepared.canonical_json.contains("Bearer raw-secret"));
    assert!(!prepared.redacted_json.contains("Bearer raw-secret"));
    assert!(!prepared.redacted_json.contains("hidden"));
    assert!(prepared.redacted_json.contains(REDACTED));

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "net.request",
            &["demo".into(), raw.into()],
        )
        .unwrap(),
        Decision::Effect(Effect::HttpRequest {
            app: "demo".into(),
            request: prepared.canonical_json.clone()
        })
    );

    let event = responded_event(
        "demo",
        prepared.request_key.clone(),
        prepared.redacted_json,
        201,
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        RecordedBody {
            kind: "inline".to_string(),
            body: "{\"saved\":true}".to_string(),
            is_base64: false,
            hash: "a".repeat(64),
            size: 14,
            mime: "application/json".to_string(),
        },
    )
    .unwrap();
    cap.fold(&mut store, &event).unwrap();
    let folded = &store.net.requests["demo"][&prepared.request_key];
    assert_eq!(folded.status, 201);
    assert_eq!(folded.body, "{\"saved\":true}");
    assert!(!folded.request_json_redacted.contains("raw-secret"));
}

#[test]
fn net_request_reserves_secret_tokens_without_resolving_them() {
    let raw = r#"{
        "url":"https://example.test/items",
        "headers":{"authorization":{"$secret":"api-token"}},
        "body":{"$secret":"payload"}
    }"#;
    let prepared = prepare_request(raw).unwrap();

    assert!(prepared.has_unresolved_secret);
    assert!(prepared.canonical_json.contains("\"$secret\":\"api-token\""));
    assert!(prepared.canonical_json.contains("\"$secret\":\"payload\""));
    assert!(prepared.redacted_json.contains("\"$secret\":\"api-token\""));
    assert!(prepared.redacted_json.contains("\"$secret\":\"payload\""));
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
    assert_eq!(
        doc.manifest.commands,
        vec!["net.fetch".to_string(), "net.request".to_string()]
    );
    assert_eq!(
        doc.manifest.events,
        vec!["net.fetched".to_string(), "net.responded".to_string()]
    );
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
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Sensitive request header values")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("169.254.169.254")));
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
