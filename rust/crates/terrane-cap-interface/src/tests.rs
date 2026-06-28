use std::any::Any;

use borsh::{BorshDeserialize, BorshSerialize};

use super::*;

#[derive(Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct SampleEvent {
    value: String,
}

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
    app_exists: QueryValue,
    replica_peer: QueryValue,
}

impl CapBus for Bus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(self.app_exists.clone()),
            ("replica", "peer") => Ok(self.replica_peer.clone()),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn event_encoding_round_trips_payloads() {
    let record = encode_event(
        "sample.changed",
        &SampleEvent {
            value: "hello".into(),
        },
    )
    .unwrap();

    assert_eq!(record.kind, "sample.changed");
    assert_eq!(
        decode_event::<SampleEvent>(&record).unwrap(),
        SampleEvent {
            value: "hello".into()
        }
    );
}

#[test]
fn namespace_and_argument_helpers_return_clear_errors() {
    assert_eq!(namespace_of("kv.set").unwrap(), "kv");
    assert!(namespace_of("bad")
        .unwrap_err()
        .to_string()
        .contains("namespace"));
    assert_eq!(arg(&["a".into()], 0, "value").unwrap(), "a");
    assert!(arg(&[], 0, "value")
        .unwrap_err()
        .to_string()
        .contains("missing value"));
}

#[test]
fn state_helpers_downcast_typed_slices() {
    let mut store = Store::default();
    state_mut::<SampleState>(&mut store, "sample")
        .unwrap()
        .value = "stored".into();

    assert_eq!(
        state_ref::<SampleState>(&store, "sample").unwrap().value,
        "stored"
    );
    assert!(state_ref::<SampleState>(&store, "missing").is_err());
}

#[test]
fn bus_helpers_validate_expected_query_types() {
    let bus = Bus {
        app_exists: QueryValue::Bool(true),
        replica_peer: QueryValue::U64(Some(42)),
    };
    assert!(app_exists(&bus, "demo").unwrap());
    assert!(ensure_app_exists(&bus, "demo").is_ok());
    assert_eq!(replica_peer(&bus).unwrap(), Some(42));

    let bad = Bus {
        app_exists: QueryValue::U64(None),
        replica_peer: QueryValue::Bool(true),
    };
    assert!(app_exists(&bad, "demo")
        .unwrap_err()
        .to_string()
        .contains("unexpected"));
    assert!(replica_peer(&bad)
        .unwrap_err()
        .to_string()
        .contains("unexpected"));
}

#[test]
fn json_extraction_and_truncation_are_deterministic() {
    assert_eq!(
        extract_json_object("prefix {\"ok\":true} suffix", "sample").unwrap(),
        "{\"ok\":true}"
    );
    assert!(extract_json_object("no json", "sample").is_err());
    assert_eq!(truncate("abcdef", 3), "abc...");
    assert_eq!(truncate("abc", 3), "abc");
}
