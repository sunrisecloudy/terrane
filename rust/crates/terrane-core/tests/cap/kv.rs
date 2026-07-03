//! Engine tests for the `kv` capability, including the broadcast-fold cascade.

use tempfile::tempdir;
use terrane_cap_kv::{KvStorageBackend, KvStorageBinding, PUBLIC_BUCKET_APP_ID};
use terrane_core::{Core, Error, ExecutionPrincipal, ReadValue, RuntimeHost, RuntimeResourceHost};
use terrane_cap_kv::DEFAULT_KV_STORAGE_PATH;

use crate::helpers::{public_req, req};

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

// ---- public bucket (kv.public.*) -------------------------------------------

#[test]
fn public_write_requires_trusted_host_authority() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    // Public authority cannot write the public bucket.
    assert_eq!(
        core.dispatch(public_req("kv.public.set", &["k", "v"])),
        Err(Error::InvalidInput(
            "kv.public.set requires trusted host authority".into()
        ))
    );
    assert_eq!(
        core.dispatch(public_req("kv.public.import", &[r#"{"k":"v"}"#])),
        Err(Error::InvalidInput(
            "kv.public.import requires trusted host authority".into()
        ))
    );
    assert!(core.state().kv.data.is_empty());

    // Trusted-host authority commits normally.
    core.dispatch(req("kv.public.set", &["k", "v"])).unwrap();
    assert_eq!(core.state().kv.data[PUBLIC_BUCKET_APP_ID]["k"], "v");
    assert_eq!(core.log_lines().unwrap().len(), 1);
}

#[test]
fn public_rm_missing_key_errors() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    assert_eq!(
        core.dispatch(req("kv.public.rm", &["ghost"])),
        Err(Error::KeyNotFound(PUBLIC_BUCKET_APP_ID.into(), "ghost".into()))
    );
}

#[test]
fn public_replay_matches_and_reopens_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("kv.public.set", &["i18n/en/system.hello", "Hello"]))
        .unwrap();
    core.dispatch(req("kv.public.import", &[r#"{"i18n/es/a":"x","i18n/de/a":"y"}"#]))
        .unwrap();
    core.dispatch(req("kv.public.rm", &["i18n/de/a"])).unwrap();

    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().kv, core.state().kv);
    let bucket = &reopened.state().kv.data[PUBLIC_BUCKET_APP_ID];
    assert_eq!(bucket.len(), 2);
    assert_eq!(bucket["i18n/en/system.hello"], "Hello");
    assert_eq!(bucket["i18n/es/a"], "x");
}

#[test]
fn public_import_is_deterministic() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    let json = r#"{"b":"2","a":"1","c":"3"}"#;
    let first = core.dispatch(req("kv.public.import", &[json])).unwrap();
    let second = core.dispatch(req("kv.public.import", &[json])).unwrap();
    assert_eq!(
        first, second,
        "identical import payloads must yield identical event records"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn public_survives_app_removed_cascade() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("kv.public.set", &["shared", "v"])).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("kv.set", &["notes", "theme", "dark"]))
        .unwrap();

    core.dispatch(req("app.remove", &["notes"])).unwrap();

    // Private app data cascaded away; public bucket intact.
    assert!(!core.state().kv.data.contains_key("notes"));
    assert_eq!(core.state().kv.data[PUBLIC_BUCKET_APP_ID]["shared"], "v");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn app_add_rejects_the_reserved_public_bucket_id() {
    // R4: no real app can own the sentinel public-bucket id, so it can never be
    // hijacked or cascade-removed by app.removed. validate_app_id rejects the
    // reserved prefix AND the '/' outright.
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    assert!(
        core.dispatch(req("app.add", &[PUBLIC_BUCKET_APP_ID, "Public"]))
            .is_err(),
        "app.add must reject the reserved public-bucket id"
    );
    assert!(core.state().app.apps.is_empty());
}

#[test]
fn public_projected_to_default_sqlite_backend() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let log = home.join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("kv.public.set", &["i18n/en/system.hello", "Hello"]))
        .unwrap();

    let sqlite_path = home.join(DEFAULT_KV_STORAGE_PATH);
    assert!(sqlite_path.is_file(), "default sqlite projection exists");
    let rows = sqlite_rows(&sqlite_path, PUBLIC_BUCKET_APP_ID);
    assert_eq!(
        rows,
        vec![(
            "i18n/en/system.hello".into(),
            "Hello".into()
        )]
    );
}

#[test]
fn apps_can_read_public_but_have_no_public_write_method() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["todo", "Todo"])).unwrap();
    core.dispatch(req("kv.public.set", &["i18n/en/todo.added", "added"]))
        .unwrap();

    // An app backend with a kv grant can read the public bucket...
    let mut host = RuntimeResourceHost::new_with_temporary_resource_grants(
        "todo",
        core.state().clone(),
        ExecutionPrincipal::local_owner(),
        vec!["kv".to_string()],
    );

    assert_eq!(
        host.read_resource("kv", "public", &["i18n/en/todo.added".to_string()])
            .unwrap(),
        ReadValue::OptString(Some("added".into()))
    );

    // ...but the kv surface it sees exposes NO write method targeting the
    // public bucket. There is no "publicSet"/"publicRm" verb to call.
    let methods = host.resource_methods("kv").unwrap();
    for method in methods {
        if method.name().starts_with("public") {
            assert!(
                matches!(method, terrane_core::ResourceMethod::Read { .. }),
                "app-visible kv surface must not expose a public write method: {:?}",
                method.name()
            );
        }
    }
}

fn sqlite_rows(path: &std::path::Path, app: &str) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT key, value FROM kv_entries WHERE app = ?1 ORDER BY key")
        .unwrap();
    stmt.query_map([app], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
