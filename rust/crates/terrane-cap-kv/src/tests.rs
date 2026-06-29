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
            ("write", "rm"),
            ("read", "scan"),
            ("read", "range"),
            ("read", "keys")
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
fn default_storage_binding_uses_sqlite_terrane_db() {
    let binding = KvStorageBinding::default();

    assert_eq!(
        binding,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(DEFAULT_KV_STORAGE_PATH.into())
        }
    );
    assert_eq!(
        binding.resolved_path(std::path::Path::new("/tmp/home")),
        Some(std::path::PathBuf::from("/tmp/home").join(DEFAULT_KV_STORAGE_PATH))
    );
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
            &["default".into(), "memory".into()],
        )
        .unwrap()
    else {
        panic!("kv.storage.set should commit");
    };
    cap.fold(&mut store, &default_events[0]).unwrap();
    assert_eq!(
        storage_plan(&store).unwrap().default,
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None
        }
    );

    let Decision::Commit(app_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.set",
            &["app".into(), "demo".into(), "memory".into()],
        )
        .unwrap()
    else {
        panic!("app kv.storage.set should commit");
    };
    cap.fold(&mut store, &app_events[0]).unwrap();
    assert_eq!(
        storage_binding(&store, Some("demo")).unwrap(),
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None
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
            backend: KvStorageBackend::Memory,
            path: None
        }
    );
}

#[test]
fn public_reads_hide_reserved_keys_and_scan_is_bounded() {
    let mut store = Store::default();
    store.kv.data.insert(
        "demo".into(),
        BTreeMap::from([
            ("a/1".into(), "one".into()),
            ("a/2".into(), "two".into()),
            ("b/1".into(), "three".into()),
            ("__terrane/rdb/v1/table/users/spec".into(), "secret".into()),
        ]),
    );
    let bus = AppBus;
    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
    };

    assert_eq!(
        KvCapability
            .read_resource(ctx, "get", &["__terrane/rdb/v1/table/users/spec".into()])
            .unwrap(),
        ReadValue::OptString(None)
    );
    assert_eq!(
        KvCapability
            .read_resource(ctx, "scan", &["a/".into(), "1".into()])
            .unwrap(),
        ReadValue::StringMap(BTreeMap::from([("a/1".into(), "one".into())]))
    );
    assert_eq!(
        KvCapability
            .read_resource(ctx, "keys", &["a/".into(), "10".into()])
            .unwrap(),
        ReadValue::StringList(vec!["a/1".into(), "a/2".into()])
    );
    assert!(KvCapability
        .read_resource(ctx, "scan", &["__terrane/".into()])
        .is_err());
}

#[test]
fn internal_helpers_can_use_reserved_prefixes() {
    let mut store = Store::default();
    let record = set_event("demo", "__terrane/rdb/v1/table/users/spec", "{}").unwrap();
    KvCapability.fold(&mut store, &record).unwrap();
    assert_eq!(
        get_value(&store, "demo", "__terrane/rdb/v1/table/users/spec").unwrap(),
        Some("{}".into())
    );
    assert_eq!(
        scan_prefix(&store, "demo", "__terrane/rdb/v1/", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn set_event_descriptions_truncate_values_for_logs() {
    let preview = "x".repeat(LOG_VALUE_PREVIEW_CHARS);
    let record = set_event(
        "demo",
        format!("{APP_BUNDLE_KEY_PREFIX}main.js"),
        format!("{preview}hidden-tail"),
    )
    .unwrap();

    let description = KvCapability.describe(&record).unwrap();

    assert_eq!(
        description,
        format!("kv.set demo/{APP_BUNDLE_KEY_PREFIX}main.js = {preview}...")
    );
    assert!(!description.contains("hidden-tail"));
}
