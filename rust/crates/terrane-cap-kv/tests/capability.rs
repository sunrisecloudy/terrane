use std::any::Any;
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Error, QueryValue, ReadValue,
    ResourceReadCtx, StateStore,
};
use terrane_cap_kv::{
    storage_binding, storage_plan, KvCapability, KvState, KvStorageBackend, KvStorageBinding,
    PUBLIC_BUCKET_APP_ID, DEFAULT_KV_STORAGE_PATH,
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

// ---- public bucket (kv.public.*) -------------------------------------------

/// Fold a committed decision's records into the store, asserting a Commit.
fn fold_commit(
    cap: &KvCapability,
    store: &mut Store,
    bus: &AppBus,
    name: &str,
    args: &[&str],
) -> Vec<terrane_cap_interface::EventRecord> {
    let decision = cap
        .decide(
            CommandCtx {
                state: store,
                bus,
            },
            name,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
    let records = match decision {
        Decision::Commit(records) => records,
        _ => panic!("{name} should commit"),
    };
    for record in &records {
        cap.fold(store, record).unwrap();
    }
    records
}

fn read_public(cap: &KvCapability, store: &Store, bus: &AppBus, method: &str, args: &[&str]) -> ReadValue {
    cap.read_resource(
        ResourceReadCtx {
            state: store,
            bus,
            app: "any-app",
        },
        method,
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn public_set_writes_public_bucket_readable_by_any_app() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    fold_commit(
        &cap,
        &mut store,
        &bus,
        "kv.public.set",
        &["i18n/en/system.hello", "Hello"],
    );

    // The value lands in the public bucket, not under the calling "app".
    assert_eq!(
        store.kv.data[PUBLIC_BUCKET_APP_ID]["i18n/en/system.hello"],
        "Hello"
    );
    assert!(!store.kv.data.contains_key("any-app"));

    // Any app can read it back through the public resource surface.
    assert_eq!(
        read_public(&cap, &store, &bus, "public", &["i18n/en/system.hello"]),
        ReadValue::OptString(Some("Hello".into()))
    );
    assert_eq!(
        read_public(&cap, &store, &bus, "publicAll", &[]),
        ReadValue::StringMap(BTreeMap::from([(
            "i18n/en/system.hello".into(),
            "Hello".into()
        )]))
    );
    assert_eq!(
        read_public(&cap, &store, &bus, "publicKeys", &["i18n/", ""]),
        ReadValue::StringList(vec!["i18n/en/system.hello".into()])
    );
    assert_eq!(
        read_public(&cap, &store, &bus, "publicScan", &["i18n/en/", ""]),
        ReadValue::StringMap(BTreeMap::from([(
            "i18n/en/system.hello".into(),
            "Hello".into()
        )]))
    );
}

#[test]
fn public_set_rejects_empty_key() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();
    assert_eq!(
        cap.decide(
            CommandCtx { state: &store, bus: &bus },
            "kv.public.set",
            &["   ".into(), "v".into()],
        )
        .unwrap_err(),
        Error::InvalidInput("key must not be empty".into())
    );
}

#[test]
fn public_rm_missing_key_errors() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();
    fold_commit(&cap, &mut store, &bus, "kv.public.set", &["k", "v"]);

    // Removing an existing public key folds a kv.deleted and empties the entry.
    fold_commit(&cap, &mut store, &bus, "kv.public.rm", &["k"]);
    assert!(
        store
            .kv
            .data
            .get(PUBLIC_BUCKET_APP_ID)
            .is_none_or(|m| m.is_empty())
    );

    // Removing a key that is not present errors (mirrors kv.rm).
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus
            },
            "kv.public.rm",
            &["ghost".into()],
        )
        .unwrap_err(),
        Error::KeyNotFound(PUBLIC_BUCKET_APP_ID.into(), "ghost".into())
    );
}

#[test]
fn public_import_emits_sorted_records_and_is_deterministic() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();

    // Deliberately unsorted input; output keys must be sorted.
    let json = r#"{"b":"2","a":"1","c":"3"}"#;
    let records_a = match cap
        .decide(
            CommandCtx { state: &store, bus: &bus },
            "kv.public.import",
            &[json.into()],
        )
        .unwrap()
    {
        Decision::Commit(r) => r,
        _ => panic!("import should commit"),
    };
    let records_b = match cap
        .decide(
            CommandCtx { state: &store, bus: &bus },
            "kv.public.import",
            &[json.into()],
        )
        .unwrap()
    {
        Decision::Commit(r) => r,
        _ => panic!("import should commit"),
    };

    assert_eq!(records_a, records_b, "identical input must yield identical events");
    // All emitted as ordinary kv.set into the public bucket.
    assert!(records_a.iter().all(|r| r.kind == "kv.set"));
    assert_eq!(records_a.len(), 3);

    // Verify sorted order by decoding each record's payload.
    let keys: Vec<String> = records_a
        .iter()
        .map(|r| {
            let e: SetView = BorshDeserialize::deserialize(&mut r.payload.as_slice()).unwrap();
            assert_eq!(e.app, PUBLIC_BUCKET_APP_ID);
            e.key
        })
        .collect();
    assert_eq!(keys, vec!["a", "b", "c"]);
}

/// Mirrors the private `Set` event layout for test-side decoding only.
#[derive(BorshDeserialize)]
struct SetView {
    app: String,
    key: String,
    value: String,
}

#[test]
fn public_import_accepts_escapes_and_unicode() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();

    let json = r#"{"k":"a\"b\\c\n\u00e9\uD83D\uDE00"}"#;
    let records = match cap
        .decide(
            CommandCtx { state: &store, bus: &bus },
            "kv.public.import",
            &[json.into()],
        )
        .unwrap()
    {
        Decision::Commit(r) => r,
        _ => panic!("import should commit"),
    };
    let e: SetView = BorshDeserialize::deserialize(&mut records[0].payload.as_slice()).unwrap();
    assert_eq!(e.value, "a\"b\\c\n\u{00e9}\u{1F600}");
}

#[test]
fn public_import_rejects_non_string_values_and_nested() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();

    for bad in ["{", r#"{"k":1}"#, r#"{"k":true}"#, r#"{"k":null}"#, r#"{"k":["a"]}"#, r#"{"k":{"a":"b"}}"#, "[]", r#""x""#] {
        assert!(
            matches!(
                cap.decide(
                    CommandCtx { state: &store, bus: &bus },
                    "kv.public.import",
                    &[bad.into()],
                ),
                Err(Error::InvalidInput(_))
            ),
            "import should reject non-flat-string-map JSON: {bad:?}"
        );
    }
}

#[test]
fn public_import_rejects_empty_key() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();
    assert_eq!(
        cap.decide(
            CommandCtx { state: &store, bus: &bus },
            "kv.public.import",
            &[r#"{"   ":"v"}"#.into()],
        )
        .unwrap_err(),
        Error::InvalidInput("public import key must not be empty".into())
    );
}

#[test]
fn public_reads_do_not_filter_reserved_key_prefixes() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();
    // A reserved-prefixed key in the public bucket is returned verbatim by
    // public reads (no is_reserved_key filtering).
    fold_commit(
        &cap,
        &mut store,
        &bus,
        "kv.public.set",
        &["__terrane/feature/flag", "on"],
    );
    assert_eq!(
        read_public(&cap, &store, &bus, "public", &["__terrane/feature/flag"]),
        ReadValue::OptString(Some("on".into()))
    );
    let all = read_public(&cap, &store, &bus, "publicAll", &[]);
    let ReadValue::StringMap(map) = all else {
        panic!("expected StringMap, got {all:?}");
    };
    assert_eq!(map["__terrane/feature/flag"], "on");
}

#[test]
fn app_scoped_reads_target_ctx_app_not_public_bucket() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();
    fold_commit(
        &cap,
        &mut store,
        &bus,
        "kv.public.set",
        &["i18n/en/system.hello", "Hello"],
    );

    // App-scoped reads use ctx.app and never see the public bucket.
    let read = cap
        .read_resource(
            ResourceReadCtx {
                state: &store,
                bus: &bus,
                app: "todo",
            },
            "all",
            &[],
        )
        .unwrap();
    let ReadValue::StringMap(map) = read else {
        panic!("expected StringMap");
    };
    assert!(
        map.is_empty(),
        "app-scoped all() must not include public keys"
    );
}

#[test]
fn public_bucket_survives_app_removed_cascade() {
    let cap = KvCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();
    fold_commit(&cap, &mut store, &bus, "kv.public.set", &["shared", "v"]);
    // Seed an app's private data, then remove it.
    store
        .kv
        .data
        .insert("demo".into(), BTreeMap::from([("x".into(), "1".into())]));
    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();

    // The private app data is gone; the public bucket is intact.
    assert!(!store.kv.data.contains_key("demo"));
    assert_eq!(
        store.kv.data[PUBLIC_BUCKET_APP_ID]["shared"],
        "v"
    );
}

#[test]
fn public_resource_methods_expose_no_write_surface() {
    // Belt-and-suspenders: the resource surface only ever lists read methods;
    // there must be no publicSet/publicRm/publicImport for app code to call.
    use terrane_cap_interface::{Capability, ResourceMethod};
    let manifest = KvCapability.manifest();
    for method in &manifest.resources {
        let m: &ResourceMethod = method;
        if m.name().starts_with("public") {
            assert!(
                matches!(m, ResourceMethod::Read { .. }),
                "public bucket must not expose a write resource method: {:?}",
                m.name()
            );
        }
    }
}
