use std::any::Any;

use terrane_cap_interface::{
    app_exists, ensure_app_exists, extract_json_object, namespace_of, replica_peer, state_mut,
    state_ref, CapBus, Error, QueryValue, StateStore,
};

#[derive(Default)]
struct SampleState {
    value: String,
}

#[derive(Default)]
struct Store {
    sample: SampleState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "sample" => Some(&self.sample),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "sample" => Some(&mut self.sample),
            _ => None,
        }
    }
}

struct Bus {
    app_exists: bool,
    peer: Option<u64>,
}

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.app_exists)),
            ("replica", "peer") => Ok(QueryValue::U64(self.peer)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn state_store_and_bus_helpers_are_usable_from_external_crates() {
    let mut store = Store::default();
    state_mut::<SampleState>(&mut store, "sample")
        .unwrap()
        .value = "ready".into();
    assert_eq!(
        state_ref::<SampleState>(&store, "sample").unwrap().value,
        "ready"
    );

    let bus = Bus {
        app_exists: true,
        peer: Some(77),
    };
    assert!(app_exists(&bus, "demo").unwrap());
    ensure_app_exists(&bus, "demo").unwrap();
    assert_eq!(replica_peer(&bus).unwrap(), Some(77));
}

#[test]
fn name_and_json_helpers_keep_external_cap_parsing_consistent() {
    assert_eq!(namespace_of("harness.run-js").unwrap(), "harness");
    assert!(namespace_of("missing-dot").is_err());
    assert_eq!(
        extract_json_object("text {\"files\":[]} text", "builder").unwrap(),
        "{\"files\":[]}"
    );
}
