use terrane_cap_interface::{CapBus, Capability, CommandCtx, QueryValue, Result, StateStore};

use terrane_cap_native::{NativeCapability, NativeState};

#[derive(Default)]
struct Store {
    native: NativeState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "native" => Some(&self.native),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "native" => Some(&mut self.native),
            _ => None,
        }
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(args.first().is_some_and(|a| a == "demo"))),
            ("replica", "peer") => Ok(QueryValue::U64(Some(42))),
            _ => Ok(QueryValue::Bool(false)),
        }
    }
}

#[test]
fn request_requires_platform_observation() {
    let store = Store::default();
    let bus = Bus;
    let ctx = CommandCtx {
        state: &store,
        bus: &bus,
    };
    let err = NativeCapability
        .decide(
            ctx,
            "native.external.open-url",
            &["demo".into(), "req-1".into(), "https://example.com".into()],
        )
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("native platform has not been observed"));
}
