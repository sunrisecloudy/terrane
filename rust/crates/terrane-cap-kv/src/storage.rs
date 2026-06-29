use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "rocksdb-storage")]
use rocksdb::{Direction, IteratorMode, Options, DB};
use rusqlite::{params, Connection};
use terrane_cap_interface::{AppId, Error, Result};

use crate::{KvState, KvStorageBackend, KvStorageBinding};

/// Rebuild all configured physical KV projections from a folded state.
pub fn sync_full_storage(home: &Path, state: &KvState) -> Result<()> {
    ensure_configured_backends(home, state)?;
    for app in state.data.keys() {
        let binding = state.storage.binding_for(Some(app));
        sync_app(home, &binding, app, state.data.get(app))?;
    }
    Ok(())
}

/// Bring physical KV projections in line with a state transition.
///
/// The event log remains the source of truth. This materializes the folded KV
/// state into the user-selected backend/location so external tools can inspect
/// the same app data without going through Terrane.
pub fn sync_storage_after_commit(home: &Path, before: &KvState, after: &KvState) -> Result<()> {
    ensure_configured_backends(home, after)?;

    let mut apps = BTreeSet::<AppId>::new();
    apps.extend(before.data.keys().cloned());
    apps.extend(after.data.keys().cloned());
    apps.extend(before.storage.apps.keys().cloned());
    apps.extend(after.storage.apps.keys().cloned());

    for app in apps {
        let before_binding = before.storage.binding_for(Some(&app));
        let after_binding = after.storage.binding_for(Some(&app));
        if before_binding != after_binding {
            sync_app(home, &before_binding, &app, None)?;
        }
        sync_app(home, &after_binding, &app, after.data.get(&app))?;
    }
    Ok(())
}

fn ensure_configured_backends(home: &Path, state: &KvState) -> Result<()> {
    ensure_backend(home, &state.storage.default)?;
    for binding in state.storage.apps.values() {
        ensure_backend(home, binding)?;
    }
    Ok(())
}

fn ensure_backend(home: &Path, binding: &KvStorageBinding) -> Result<()> {
    match binding.backend {
        KvStorageBackend::Memory => Ok(()),
        KvStorageBackend::Sqlite => ensure_sqlite_backend(home, binding),
        KvStorageBackend::RocksDb => ensure_rocksdb_backend(home, binding),
    }
}

fn sync_app(
    home: &Path,
    binding: &KvStorageBinding,
    app: &str,
    data: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    match binding.backend {
        KvStorageBackend::Memory => Ok(()),
        KvStorageBackend::Sqlite => sync_sqlite_backend(home, binding, app, data),
        KvStorageBackend::RocksDb => sync_rocksdb_backend(home, binding, app, data),
    }
}

fn storage_path(home: &Path, binding: &KvStorageBinding) -> Result<PathBuf> {
    binding
        .resolved_path(home)
        .ok_or_else(|| Error::Storage("memory backend has no storage path".into()))
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| Error::Storage(e.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(not(feature = "rocksdb-storage"))]
fn unavailable_backend(backend: &KvStorageBackend) -> Result<()> {
    let feature = backend.required_feature().unwrap_or("unknown");
    Err(Error::Storage(format!(
        "kv storage backend {} requires feature {feature}",
        backend.as_str()
    )))
}

fn ensure_sqlite_backend(home: &Path, binding: &KvStorageBinding) -> Result<()> {
    let path = storage_path(home, binding)?;
    let conn = open_sqlite(&path)?;
    ensure_sqlite_schema(&conn)
}

fn sync_sqlite_backend(
    home: &Path,
    binding: &KvStorageBinding,
    app: &str,
    data: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let path = storage_path(home, binding)?;
    sync_sqlite_app(&path, app, data)
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    ensure_parent(path)?;
    Connection::open(path).map_err(|e| Error::Storage(format!("open sqlite: {e}")))
}

fn ensure_sqlite_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv_entries (
            app TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (app, key)
        );",
    )
    .map_err(|e| Error::Storage(format!("init sqlite schema: {e}")))
}

fn sync_sqlite_app(path: &Path, app: &str, data: Option<&BTreeMap<String, String>>) -> Result<()> {
    let mut conn = open_sqlite(path)?;
    ensure_sqlite_schema(&conn)?;
    let tx = conn
        .transaction()
        .map_err(|e| Error::Storage(format!("begin sqlite transaction: {e}")))?;
    tx.execute("DELETE FROM kv_entries WHERE app = ?1", params![app])
        .map_err(|e| Error::Storage(format!("delete sqlite app rows: {e}")))?;
    if let Some(data) = data {
        for (key, value) in data {
            tx.execute(
                "INSERT INTO kv_entries (app, key, value) VALUES (?1, ?2, ?3)",
                params![app, key, value],
            )
            .map_err(|e| Error::Storage(format!("insert sqlite row: {e}")))?;
        }
    }
    tx.commit()
        .map_err(|e| Error::Storage(format!("commit sqlite transaction: {e}")))
}

#[cfg(feature = "rocksdb-storage")]
fn ensure_rocksdb_backend(home: &Path, binding: &KvStorageBinding) -> Result<()> {
    let path = storage_path(home, binding)?;
    open_rocksdb(&path).map(|_| ())
}

#[cfg(not(feature = "rocksdb-storage"))]
fn ensure_rocksdb_backend(_home: &Path, binding: &KvStorageBinding) -> Result<()> {
    unavailable_backend(&binding.backend)
}

#[cfg(feature = "rocksdb-storage")]
fn sync_rocksdb_backend(
    home: &Path,
    binding: &KvStorageBinding,
    app: &str,
    data: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let path = storage_path(home, binding)?;
    sync_rocksdb_app(&path, app, data)
}

#[cfg(not(feature = "rocksdb-storage"))]
fn sync_rocksdb_backend(
    _home: &Path,
    binding: &KvStorageBinding,
    _app: &str,
    _data: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    unavailable_backend(&binding.backend)
}

#[cfg(feature = "rocksdb-storage")]
fn open_rocksdb(path: &Path) -> Result<DB> {
    fs::create_dir_all(path).map_err(|e| Error::Storage(e.to_string()))?;
    let mut opts = Options::default();
    opts.create_if_missing(true);
    DB::open(&opts, path).map_err(|e| Error::Storage(format!("open rocksdb: {e}")))
}

#[cfg(feature = "rocksdb-storage")]
fn sync_rocksdb_app(path: &Path, app: &str, data: Option<&BTreeMap<String, String>>) -> Result<()> {
    let db = open_rocksdb(path)?;
    let prefix = rocksdb_app_prefix(app);
    let keys = db
        .iterator(IteratorMode::From(&prefix, Direction::Forward))
        .take_while(|item| match item {
            Ok((key, _)) => key.starts_with(&prefix),
            Err(_) => true,
        })
        .map(|item| {
            item.map(|(key, _)| key.to_vec())
                .map_err(|e| Error::Storage(format!("scan rocksdb app rows: {e}")))
        })
        .collect::<Result<Vec<_>>>()?;
    for key in keys {
        db.delete(key)
            .map_err(|e| Error::Storage(format!("delete rocksdb row: {e}")))?;
    }
    if let Some(data) = data {
        for (key, value) in data {
            db.put(rocksdb_key(app, key), value.as_bytes())
                .map_err(|e| Error::Storage(format!("put rocksdb row: {e}")))?;
        }
    }
    Ok(())
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
