use std::any::Any;

use terrane_cap_interface::{CapBus, QueryValue};

use super::*;

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

struct AppBus;

impl CapBus for AppBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn fetched_event_describes_and_folds_response() {
    let mut store = Store::default();
    let cap = NetCapability;
    let record = fetched_event("demo", "https://example.test", 200, "ok".into()).unwrap();

    let description = cap.describe(&record).unwrap();
    assert!(description.contains("net.fetched demo https://example.test"));
    assert!(description.contains("200 (2 bytes)"));
    cap.fold(&mut store, &record).unwrap();
    let response = &store.net.fetches["demo"]["https://example.test"];
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "ok");
}

#[test]
fn fetch_decision_validates_url_without_running_network() {
    let store = Store::default();
    let bus = AppBus;

    assert_eq!(
        NetCapability
            .decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                "net.fetch",
                &["demo".into(), "https://example.test".into()],
            )
            .unwrap(),
        Decision::Effect(Effect::HttpGet {
            app: "demo".into(),
            url: "https://example.test".into()
        })
    );
    assert_eq!(
        NetCapability
            .decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                "net.fetch",
                &["demo".into(), "".into()],
            )
            .unwrap_err(),
        Error::InvalidInput("url must not be empty".into())
    );
}
