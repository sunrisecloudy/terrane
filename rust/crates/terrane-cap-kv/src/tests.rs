use std::any::Any;
use std::collections::BTreeMap;

use terrane_cap_interface::{CapBus, QueryValue};

use super::*;

#[derive(Default)]
struct Store {
    kv: KvState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "kv" => Some(&self.kv),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "kv" => Some(&mut self.kv),
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
fn resource_manifest_exposes_expected_backend_methods() {
    let names: Vec<_> = KvCapability
        .resource_api()
        .into_iter()
        .map(|method| (method.kind(), method.name()))
        .collect();

    assert_eq!(
        names,
        vec![
            ("write", "set"),
            ("read", "get"),
            ("read", "all"),
            ("write", "rm")
        ]
    );
}

#[test]
fn resource_reads_return_values_for_current_app() {
    let mut store = Store::default();
    store.kv.data.insert(
        "demo".into(),
        BTreeMap::from([("answer".into(), "42".into())]),
    );
    let bus = AppBus;
    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
    };

    assert_eq!(
        KvCapability
            .read_resource(ctx, "get", &["answer".into()])
            .unwrap(),
        ReadValue::OptString(Some("42".into()))
    );
    assert_eq!(
        KvCapability.read_resource(ctx, "all", &[]).unwrap(),
        ReadValue::StringMap(BTreeMap::from([("answer".into(), "42".into())]))
    );
}

#[test]
fn set_rejects_empty_keys_before_recording_event() {
    let store = Store::default();
    let bus = AppBus;
    let err = KvCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.set",
            &["demo".into(), "".into(), "value".into()],
        )
        .unwrap_err();

    assert_eq!(err, Error::InvalidInput("key must not be empty".into()));
}

#[test]
fn storage_bindings_are_owned_by_kv_capability() {
    let cap = KvCapability;
    let bus = AppBus;
    let mut store = Store::default();

    let Decision::Commit(default_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.set",
            &[
                "default".into(),
                "sqlite".into(),
                ".terrane/kv.sqlite3".into(),
            ],
        )
        .unwrap()
    else {
        panic!("kv.storage.set should commit");
    };
    cap.fold(&mut store, &default_events[0]).unwrap();
    assert_eq!(
        storage_plan(&store).unwrap().default,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(".terrane/kv.sqlite3".into())
        }
    );

    let Decision::Commit(app_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.set",
            &[
                "app".into(),
                "demo".into(),
                "rocksdb".into(),
                "/tmp/demo.rocksdb".into(),
            ],
        )
        .unwrap()
    else {
        panic!("app kv.storage.set should commit");
    };
    cap.fold(&mut store, &app_events[0]).unwrap();
    assert_eq!(
        storage_binding(&store, Some("demo")).unwrap(),
        KvStorageBinding {
            backend: KvStorageBackend::RocksDb,
            path: Some("/tmp/demo.rocksdb".into())
        }
    );

    let Decision::Commit(clear_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.clear",
            &["app".into(), "demo".into()],
        )
        .unwrap()
    else {
        panic!("kv.storage.clear should commit");
    };
    cap.fold(&mut store, &clear_events[0]).unwrap();
    assert_eq!(
        storage_binding(&store, Some("demo")).unwrap(),
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(".terrane/kv.sqlite3".into())
        }
    );
}
