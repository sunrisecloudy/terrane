use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_browser::request::{prepare_render, MAX_WAIT_MS};
use terrane_cap_browser::{
    rendered_event, BrowserCapability, BrowserState, RecordedBody, RenderedEvent,
};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};

#[derive(Default)]
struct Store {
    browser: BrowserState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "browser" => Some(&self.browser),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "browser" => Some(&mut self.browser),
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
fn browser_render_canonicalizes_redacts_and_returns_effect() {
    let cap = BrowserCapability;
    let store = Store::default();
    let raw = r#"{
        "url":"https://example.test/dashboard?token=secret#frag",
        "output":"html",
        "waitMs":1000,
        "viewport":{"h":600,"w":900},
        "allowedHosts":["example.test"],
        "sensitiveHeaders":["authorization"]
    }"#;
    let prepared = prepare_render(raw).unwrap();

    assert!(prepared.canonical_json.contains("token=secret"));
    assert!(!prepared.redacted_json.contains("token=secret"));
    assert!(prepared.redacted_json.contains("?<redacted>"));
    assert_eq!(prepared.request_key.len(), 64);

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: true },
            },
            "browser.render",
            &["demo".into(), raw.into()],
        )
        .unwrap(),
        Decision::Effect(Effect::BrowserRender {
            app: "demo".into(),
            request: prepared.canonical_json,
        })
    );
}

#[test]
fn browser_peek_is_transient_and_render_is_rate_limited_for_resources() {
    let cap = BrowserCapability;
    let store = Store::default();
    let raw = r#"{"url":"https://example.test","output":"text"}"#;
    let prepared = prepare_render(raw).unwrap();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: true },
            },
            "browser.peek",
            &["demo".into(), raw.into()],
        )
        .unwrap(),
        Decision::TransientEffect(Effect::BrowserRender {
            app: "demo".into(),
            request: prepared.canonical_json,
        })
    );
    let cap_limit = cap.recorded_call_per_run_limit("render").unwrap();
    assert_eq!(cap_limit.limit, 30);
    assert!(cap.recorded_call_per_run_limit("peek").is_none());
}

#[test]
fn browser_folds_recorded_render_and_cleans_removed_app() {
    let cap = BrowserCapability;
    let mut store = Store::default();
    let raw = r#"{"url":"https://example.test","output":"text"}"#;
    let prepared = prepare_render(raw).unwrap();
    let event = rendered_event(RenderedEvent {
        app: "demo".to_string(),
        request_key: prepared.request_key.clone(),
        request_json_redacted: prepared.redacted_json,
        url: prepared.url,
        output: "text".to_string(),
        status: 200,
        body: RecordedBody {
            kind: "inline".to_string(),
            body: "Rendered text".to_string(),
            hash: "a".repeat(64),
            size: 13,
            mime: "text/plain; charset=utf-8".to_string(),
        },
        title: "Demo".to_string(),
    })
    .unwrap();

    cap.fold(&mut store, &event).unwrap();
    assert_eq!(
        store.browser.renders["demo"][&prepared.request_key].body,
        "Rendered text"
    );

    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(store.browser.renders.is_empty());
}

#[test]
fn browser_rejects_invalid_inputs() {
    assert!(prepare_render("[]").is_err());
    assert!(prepare_render(r#"{"url":""}"#).is_err());
    assert!(prepare_render(r#"{"url":"https://example.test","output":"video"}"#).is_err());
    assert!(prepare_render(&format!(
        r#"{{"url":"https://example.test","waitMs":{}}}"#,
        MAX_WAIT_MS + 1
    ))
    .is_err());
    assert!(prepare_render(
        r#"{"url":"https://example.test","viewport":{"w":3841,"h":800}}"#
    )
    .is_err());
}

#[test]
fn browser_doc_covers_replay_security_and_limits() {
    let doc = BrowserCapability.doc(false);

    assert_eq!(doc.namespace, "browser");
    assert_eq!(doc.manifest.commands, vec!["browser.render".to_string()]);
    assert_eq!(doc.manifest.events, vec!["browser.rendered".to_string()]);
    assert_eq!(doc.manifest.subscriptions, vec!["app.removed".to_string()]);
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("never by replay")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Ephemeral profiles")));
}
