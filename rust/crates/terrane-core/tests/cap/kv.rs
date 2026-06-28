//! Engine tests for the `kv` capability, including the broadcast-fold cascade.

use tempfile::tempdir;
use terrane_core::cap::kv::{KvStorageBackend, KvStorageBinding};
use terrane_core::Core;
use terrane_core::Error;

use crate::helpers::req;

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

    core.dispatch(req(
        "kv.storage.set",
        &["default", "sqlite", ".terrane/kv.sqlite3"],
    ))
    .unwrap();
    core.dispatch(req(
        "kv.storage.set",
        &["app", "notes", "rocksdb", "/tmp/notes.rocksdb"],
    ))
    .unwrap();

    assert_eq!(
        core.kv_storage_plan().default,
        KvStorageBinding {
            backend: KvStorageBackend::Sqlite,
            path: Some(".terrane/kv.sqlite3".into())
        }
    );
    assert_eq!(
        core.kv_storage_plan().apps["notes"],
        KvStorageBinding {
            backend: KvStorageBackend::RocksDb,
            path: Some("/tmp/notes.rocksdb".into())
        }
    );
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.kv_storage_plan(), core.kv_storage_plan());

    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(!core.kv_storage_plan().apps.contains_key("notes"));
}
