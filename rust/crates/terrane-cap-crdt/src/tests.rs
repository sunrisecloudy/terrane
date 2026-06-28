use std::any::Any;

use terrane_cap_interface::{CapBus, QueryValue};

use super::*;

#[derive(Default)]
struct Store {
    crdt: CrdtState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "crdt" => Some(&self.crdt),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "crdt" => Some(&mut self.crdt),
            _ => None,
        }
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            ("replica", "peer") => Ok(QueryValue::U64(Some(100))),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn hex_encoding_is_lowercase_and_reversible_through_merge_validation() {
    assert_eq!(to_hex(&[0, 10, 255]), "000aff");

    let store = Store::default();
    let bus = Bus;
    assert!(CrdtCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "crdt.merge",
            &["demo".into(), "abc".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("odd-length"));
}

#[test]
fn map_set_decision_records_update_without_mutating_live_state() {
    let store = Store::default();
    let bus = Bus;
    let decision = CrdtCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "crdt.mapSet",
            &["demo".into(), "profile".into(), "name".into(), "Ada".into()],
        )
        .unwrap();

    let Decision::Commit(events) = decision else {
        panic!("expected crdt.mapSet to commit an update");
    };
    assert_eq!(events.len(), 1);
    assert!(!store.crdt.docs.contains_key("demo"));
}

#[test]
fn resource_manifest_includes_map_list_and_text_methods() {
    let names: Vec<_> = CrdtCapability
        .resource_api()
        .into_iter()
        .map(|method| method.name())
        .collect();

    assert!(names.contains(&"mapSet"));
    assert!(names.contains(&"listPush"));
    assert!(names.contains(&"textGet"));
}
