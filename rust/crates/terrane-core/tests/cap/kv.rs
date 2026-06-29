//! Engine tests for the `kv` capability, including the broadcast-fold cascade.

use tempfile::tempdir;
use terrane_cap_kv::{KvStorageBackend, KvStorageBinding, DEFAULT_KV_STORAGE_PATH};
use terrane_core::Core;
use terrane_core::Error;

use crate::helpers::req;

#[test]
fn default_kv_storage_uses_sqlite_terrane_db_relative_to_log_home() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let core = Core::open(&log).unwrap();

    assert_eq!(
        core.kv_storage_plan().default,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(DEFAULT_KV_STORAGE_PATH.into())
        }
    );
    assert!(
        dir.path().join(DEFAULT_KV_STORAGE_PATH).is_file(),
        "default sqlite projection should be created next to the log"
    );
}

#[test]
fn kv_records_and_cascades_via_broadcast_fold() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    // Writing to an app that doesn't exist is rejected.
    assert_eq!(
        core.dispatch(req("kv.set", &["ghost", "k", "v"])),
        Err(Error::AppNotFound("ghost".into()))
    );

    core.dispatch(req("kv.set", &["notes", "theme", "dark"]))
        .unwrap();
    assert_eq!(core.state().kv.data["notes"]["theme"], "dark");
    assert!(core.replay_matches().unwrap());

    // Deleting a missing key errors.
    assert_eq!(
        core.dispatch(req("kv.rm", &["notes", "ghost"])),
        Err(Error::KeyNotFound("notes".into(), "ghost".into()))
    );

    // Removing the app cascades to its data — the kv capability reacts to the
    // app.removed event via broadcast fold, with no app→kv coupling.
    core.dispatch(req("kv.set", &["notes", "lang", "en"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(core.state().kv.data.is_empty());
    assert!(Core::open(&log).unwrap().state().kv.data.is_empty());
}

#[test]
fn kv_storage_plan_is_cap_owned_and_replayed_for_core_use() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    core.dispatch(req("kv.storage.set", &["default", "memory"]))
        .unwrap();
    core.dispatch(req("kv.storage.set", &["app", "notes", "memory"]))
        .unwrap();

    assert_eq!(
        core.kv_storage_plan().default,
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None
        }
    );
    assert_eq!(
        core.kv_storage_plan().apps["notes"],
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None
        }
    );
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.kv_storage_plan(), core.kv_storage_plan());

    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(!core.kv_storage_plan().apps.contains_key("notes"));
}

#[test]
fn sqlite_storage_backend_is_available_by_default() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let sqlite = dir.path().join("notes.sqlite3");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req(
        "kv.storage.set",
        &["app", "notes", "sqlite", sqlite.to_str().unwrap()],
    ))
    .unwrap();
    core.dispatch(req("kv.set", &["notes", "theme", "dark"]))
        .unwrap();

    assert!(sqlite.is_file(), "sqlite projection should exist on disk");
    assert_eq!(
        core.kv_storage_plan().apps["notes"],
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(sqlite.to_str().unwrap().to_string())
        }
    );
    assert!(core.replay_matches().unwrap());
}

#[cfg(not(feature = "rocksdb-storage"))]
#[test]
fn rocksdb_storage_requires_feature_before_commit() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    assert_eq!(
        core.dispatch(req("kv.storage.set", &["app", "notes", "rocksdb"])),
        Err(Error::InvalidInput(
            "kv storage backend rocksdb requires feature rocksdb-storage".into()
        ))
    );
    assert_eq!(core.log_lines().unwrap().len(), 1);
}
