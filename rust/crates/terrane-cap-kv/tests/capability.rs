use std::any::Any;
use std::collections::BTreeMap;

use borsh::BorshSerialize;
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Error, QueryValue, ReadValue,
    ResourceReadCtx, StateStore,
};
use terrane_cap_kv::{
    storage_binding, storage_plan, KvCapability, KvState, KvStorageBackend, KvStorageBinding,
    DEFAULT_KV_STORAGE_PATH,
};

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
fn kv_capability_sets_reads_deletes_and_drops_removed_app_data() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    let Decision::Commit(set_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.set",
            &[
                "demo".into(),
                "greeting".into(),
                "hello".into(),
                "world".into(),
            ],
        )
        .unwrap()
    else {
        panic!("kv.set should commit");
    };
    cap.fold(&mut store, &set_events[0]).unwrap();

    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
        host: None,
    };
    assert_eq!(
        cap.read_resource(ctx, "get", &["greeting".into()]).unwrap(),
        ReadValue::OptString(Some("hello world".into()))
    );
    assert_eq!(
        cap.read_resource(ctx, "all", &[]).unwrap(),
        ReadValue::StringMap(BTreeMap::from([("greeting".into(), "hello world".into())]))
    );

    let Decision::Commit(delete_events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.rm",
            &["demo".into(), "greeting".into()],
        )
        .unwrap()
    else {
        panic!("kv.rm should commit");
    };
    cap.fold(&mut store, &delete_events[0]).unwrap();
    assert!(store.kv.data.is_empty());

    store.kv.data.insert(
        "demo".into(),
        BTreeMap::from([("left".into(), "value".into())]),
    );
    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(store.kv.data.is_empty());
}

#[test]
fn kv_capability_requires_existing_app_and_key_for_writes() {
    let cap = KvCapability;
    let store = Store::default();
    let missing_app = AppBus { exists: false };

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &missing_app,
            },
            "kv.set",
            &["demo".into(), "key".into(), "value".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("demo".into())
    );

    let bus = AppBus { exists: true };
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.rm",
            &["demo".into(), "missing".into()],
        )
        .unwrap_err(),
        Error::KeyNotFound("demo".into(), "missing".into())
    );
}

#[test]
fn kv_default_storage_is_sqlite_at_terrane_db() {
    let store = Store::default();

    assert_eq!(
        storage_plan(&store).unwrap().default,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(DEFAULT_KV_STORAGE_PATH.into())
        }
    );
}

#[test]
fn kv_capability_records_user_storage_bindings() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
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

    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert_eq!(
        storage_binding(&store, Some("demo")).unwrap(),
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None
        }
    );
}

#[test]
fn kv_capability_accepts_sqlite_storage_by_default() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    let Decision::Commit(events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.set",
            &["default".into(), "sqlite".into(), "kv.sqlite3".into()],
        )
        .unwrap()
    else {
        panic!("sqlite kv.storage.set should commit");
    };
    cap.fold(&mut store, &events[0]).unwrap();
    assert_eq!(
        storage_plan(&store).unwrap().default,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some("kv.sqlite3".into())
        }
    );
}

#[cfg(not(feature = "rocksdb-storage"))]
#[test]
fn kv_capability_rejects_rocksdb_storage_without_feature() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "kv.storage.set",
            &["app".into(), "demo".into(), "rocksdb".into()],
        )
        .unwrap_err(),
        Error::InvalidInput("kv storage backend rocksdb requires feature rocksdb-storage".into())
    );
}
