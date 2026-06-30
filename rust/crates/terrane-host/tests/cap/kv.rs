//! e2e smoke for `kv`. Logic detail is covered by `rust/crates/terrane-core/tests/cap/kv.rs`.

use std::path::{Path, PathBuf};

#[cfg(feature = "rocksdb-storage")]
use rocksdb::{Direction, IteratorMode, Options, DB};
use rusqlite::{Connection, OptionalExtension};
use tempfile::tempdir;

use crate::helpers::terrane;

/// Absolute path to a repo app bundle (`apps/<name>`).
fn app_source(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")) // .../rust/crates/terrane-host
        .join("../../../apps")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("apps/{name} bundle exists"))
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn kv_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);

    let (ok, out, err) = terrane(home, &["kv", "set", "notes", "theme", "dark"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("kv.set"), "out: {out}");
}

#[test]
fn sqlite_storage_projection_is_externally_queryable_after_each_kv_operation() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("todo-cli");
    let sqlite_path = home.join("todo-cli.sqlite3");

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "todo-cli", "Todo CLI", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "todo-cli", "kv"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "kv",
            "storage",
            "set",
            "--app",
            "todo-cli",
            "sqlite",
            "--path",
            sqlite_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "stderr: {err}");
    assert!(out.contains("kv.storage.configured"), "out: {out}");
    assert!(sqlite_path.is_file(), "sqlite file was created");
    assert_eq!(
        sqlite_rows(&sqlite_path, "todo-cli"),
        Vec::<(String, String)>::new()
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "buy milk"]);
    assert!(ok, "host run add failed: {err}");
    assert_eq!(out.trim(), "added #1 buy milk");
    assert_eq!(
        sqlite_value(&sqlite_path, "todo-cli", "seq"),
        Some("1".into())
    );
    assert_eq!(
        sqlite_value(&sqlite_path, "todo-cli", "item:1"),
        Some("buy milk".into())
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "list"]);
    assert!(ok, "host run list failed: {err}");
    assert_eq!(out.trim(), "#1 buy milk");
    assert_eq!(
        sqlite_rows(&sqlite_path, "todo-cli"),
        vec![
            ("item:1".into(), "buy milk".into()),
            ("seq".into(), "1".into())
        ]
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "ship it"]);
    assert!(ok, "host run add #2 failed: {err}");
    assert_eq!(out.trim(), "added #2 ship it");
    assert_eq!(
        sqlite_value(&sqlite_path, "todo-cli", "seq"),
        Some("2".into())
    );
    assert_eq!(
        sqlite_value(&sqlite_path, "todo-cli", "item:2"),
        Some("ship it".into())
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "done", "1"]);
    assert!(ok, "host run done failed: {err}");
    assert_eq!(out.trim(), "done #1");
    assert_eq!(sqlite_value(&sqlite_path, "todo-cli", "item:1"), None);
    assert_eq!(
        sqlite_rows(&sqlite_path, "todo-cli"),
        vec![
            ("item:2".into(), "ship it".into()),
            ("seq".into(), "2".into())
        ]
    );

    let (ok, out, err) = terrane(home, &["kv", "delete", "todo-cli", "item:2"]);
    assert!(ok, "kv.delete failed: {err}");
    assert!(out.contains("kv.deleted"), "out: {out}");
    assert_eq!(sqlite_value(&sqlite_path, "todo-cli", "item:2"), None);
    assert_eq!(
        sqlite_rows(&sqlite_path, "todo-cli"),
        vec![("seq".into(), "2".into())]
    );

    let (ok, out, err) = terrane(home, &["kv", "storage", "clear", "--app", "todo-cli"]);
    assert!(ok, "kv storage clear failed: {err}");
    assert!(out.contains("kv.storage.cleared"), "out: {out}");
    assert_eq!(
        sqlite_rows(&sqlite_path, "todo-cli"),
        Vec::<(String, String)>::new()
    );
}

#[cfg(not(feature = "rocksdb-storage"))]
#[test]
fn rocksdb_storage_projection_requires_feature() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "todo-cli", "Todo CLI"]);

    let (ok, out, err) = terrane(
        home,
        &["kv", "storage", "set", "--app", "todo-cli", "rocksdb"],
    );

    assert!(!ok, "rocksdb storage should be feature gated: {out}");
    assert!(
        err.contains("kv storage backend rocksdb requires feature rocksdb-storage"),
        "stderr: {err}"
    );
}

#[cfg(feature = "rocksdb-storage")]
#[test]
fn rocksdb_storage_projection_is_externally_queryable_after_each_kv_operation() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("todo-cli");
    let rocks_path = home.join("todo-cli.rocksdb");

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "todo-cli", "Todo CLI", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "todo-cli", "kv"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "kv",
            "storage",
            "set",
            "--app",
            "todo-cli",
            "rocksdb",
            "--path",
            rocks_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "stderr: {err}");
    assert!(out.contains("kv.storage.configured"), "out: {out}");
    assert!(rocks_path.is_dir(), "rocksdb directory was created");
    assert_eq!(
        rocksdb_rows(&rocks_path, "todo-cli"),
        Vec::<(String, String)>::new()
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "buy milk"]);
    assert!(ok, "host run add failed: {err}");
    assert_eq!(out.trim(), "added #1 buy milk");
    assert!(
        matches!(rocksdb_value(&rocks_path, "todo-cli", "item:1"), Some(value) if value == "buy milk")
    );
    assert_eq!(
        rocksdb_value(&rocks_path, "todo-cli", "seq"),
        Some("1".into())
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "list"]);
    assert!(ok, "host run list failed: {err}");
    assert_eq!(out.trim(), "#1 buy milk");
    assert_eq!(
        rocksdb_rows(&rocks_path, "todo-cli"),
        vec![
            ("item:1".into(), "buy milk".into()),
            ("seq".into(), "1".into())
        ]
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "ship it"]);
    assert!(ok, "host run add #2 failed: {err}");
    assert_eq!(out.trim(), "added #2 ship it");
    assert_eq!(
        rocksdb_value(&rocks_path, "todo-cli", "item:2"),
        Some("ship it".into())
    );
    assert_eq!(
        rocksdb_value(&rocks_path, "todo-cli", "seq"),
        Some("2".into())
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "done", "1"]);
    assert!(ok, "host run done failed: {err}");
    assert_eq!(out.trim(), "done #1");
    assert_eq!(rocksdb_value(&rocks_path, "todo-cli", "item:1"), None);
    assert_eq!(
        rocksdb_rows(&rocks_path, "todo-cli"),
        vec![
            ("item:2".into(), "ship it".into()),
            ("seq".into(), "2".into())
        ]
    );

    let (ok, out, err) = terrane(home, &["kv", "delete", "todo-cli", "item:2"]);
    assert!(ok, "kv.delete failed: {err}");
    assert!(out.contains("kv.deleted"), "out: {out}");
    assert_eq!(rocksdb_value(&rocks_path, "todo-cli", "item:2"), None);
    assert_eq!(
        rocksdb_rows(&rocks_path, "todo-cli"),
        vec![("seq".into(), "2".into())]
    );

    let (ok, out, err) = terrane(home, &["kv", "storage", "clear", "--app", "todo-cli"]);
    assert!(ok, "kv storage clear failed: {err}");
    assert!(out.contains("kv.storage.cleared"), "out: {out}");
    assert_eq!(
        rocksdb_rows(&rocks_path, "todo-cli"),
        Vec::<(String, String)>::new()
    );
}

fn sqlite_value(path: &Path, app: &str, key: &str) -> Option<String> {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT value FROM kv_entries WHERE app = ?1 AND key = ?2",
            [app, key],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn sqlite_rows(path: &Path, app: &str) -> Vec<(String, String)> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT key, value FROM kv_entries WHERE app = ?1 ORDER BY key")
        .unwrap();
    stmt.query_map([app], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[cfg(feature = "rocksdb-storage")]
fn rocksdb_value(path: &Path, app: &str, key: &str) -> Option<String> {
    let db = open_rocksdb_read_only(path);
    db.get(rocksdb_key(app, key))
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

#[cfg(feature = "rocksdb-storage")]
fn rocksdb_rows(path: &Path, app: &str) -> Vec<(String, String)> {
    let db = open_rocksdb_read_only(path);
    let prefix = rocksdb_app_prefix(app);
    db.iterator(IteratorMode::From(&prefix, Direction::Forward))
        .take_while(|item| match item {
            Ok((key, _)) => key.starts_with(&prefix),
            Err(_) => true,
        })
        .map(|item| {
            let (key, value) = item.unwrap();
            (
                String::from_utf8(key[prefix.len()..].to_vec()).unwrap(),
                String::from_utf8(value.to_vec()).unwrap(),
            )
        })
        .collect()
}

#[cfg(feature = "rocksdb-storage")]
fn open_rocksdb_read_only(path: &Path) -> DB {
    let opts = Options::default();
    DB::open_for_read_only(&opts, path, false).unwrap()
}

#[cfg(feature = "rocksdb-storage")]
fn rocksdb_app_prefix(app: &str) -> Vec<u8> {
    let app_bytes = app.as_bytes();
    let mut out = Vec::with_capacity(4 + app_bytes.len());
    out.extend_from_slice(&(app_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(app_bytes);
    out
}

#[cfg(feature = "rocksdb-storage")]
fn rocksdb_key(app: &str, key: &str) -> Vec<u8> {
    let mut out = rocksdb_app_prefix(app);
    out.extend_from_slice(key.as_bytes());
    out
}
