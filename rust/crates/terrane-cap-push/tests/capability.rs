use terrane_cap_interface::{CapBus, Capability, CommandCtx, QueryValue, Result, StateStore};
use terrane_cap_push::{matches_pattern, render_template, PushCapability, PushState};

#[derive(Default)]
struct Store {
    push: PushState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "push" => Some(&self.push),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "push" => Some(&mut self.push),
            _ => None,
        }
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(args.first().is_some_and(|a| a == "demo"))),
            _ => Ok(QueryValue::Bool(false)),
        }
    }
}

#[test]
fn validates_patterns_and_templates() {
    let store = Store::default();
    let bus = Bus;
    let ctx = CommandCtx {
        state: &store,
        bus: &bus,
    };
    let err = PushCapability
        .decide(
            ctx,
            "push.subscribe",
            &["demo".into(), "*".into(), "Title".into()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("event_pattern must be"));

    let ctx = CommandCtx {
        state: &store,
        bus: &bus,
    };
    let err = PushCapability
        .decide(
            ctx,
            "push.subscribe",
            &["demo".into(), "kv.*".into(), "Title {key".into()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("unmatched"));
}

#[test]
fn pattern_matching_and_rendering_are_pure() {
    assert!(matches_pattern("kv.*", "kv.set"));
    assert!(matches_pattern("kv.set", "kv.set"));
    assert!(!matches_pattern("kv.*", "crdt.update"));

    let record = terrane_cap_kv::set_event("demo", "color", "blue").unwrap();
    let (title, body) =
        render_template("Changed {key}|{kind} = {value}", &record, Some("ignored")).unwrap();
    assert_eq!(title, "Changed color");
    assert_eq!(body, "kv.set = blue");
}
