//! The deterministic table-copy helpers (the bundle's `meta` header + the
//! syncable tables, each ordered by its spec ordering columns) — the byte-stable
//! re-export substrate (DL-24).

use super::guard::is_local_only_namespace;
use super::policy::{ExportOptions, EXPORT_FORMAT_VERSION, STORAGE_SCHEMA_VERSION};
use crate::map_sql;
use forge_domain::Result;
use rusqlite::{params, Connection};

/// Write the bundle's `meta` header rows (format version + workspace id) in a
/// fixed key order. Values are stored as utf-8 text bytes, matching how the
/// fixtures model `meta` rows (`[key, value]` strings).
pub(super) fn write_meta_header(bundle: &Connection, options: &ExportOptions) -> Result<()> {
    // Deterministic key order. `set_meta` is a plain upsert into the bundle's
    // already-created `meta` table.
    set_meta(
        bundle,
        "export_format_version",
        &EXPORT_FORMAT_VERSION.to_string(),
    )?;
    set_meta(
        bundle,
        "forge_storage_schema_version",
        &STORAGE_SCHEMA_VERSION.to_string(),
    )?;
    set_meta(bundle, "workspace_id", &options.workspace_id)?;
    Ok(())
}

/// Upsert one `meta` row (utf-8 text value).
fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value, updated_at) VALUES (?1, ?2, 0)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value.as_bytes()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Copy the portable `kv` rows from `src` to `dst`, ordered by `(namespace,
/// key)` and **excluding** local-only / secret namespaces
/// ([`is_local_only_namespace`]). Tombstones are retained (a deleted key must
/// round-trip as deleted, DL-21), so the bundle carries the full sync-correct kv
/// shape minus secrets.
pub(super) fn copy_kv(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT namespace, key, value, content_type, logical_version, updated_at, tombstone
               FROM kv ORDER BY namespace, key",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (namespace, key, value, content_type, logical_version, updated_at, tombstone) =
            r.map_err(map_sql)?;
        // EXCLUSION GUARD: a secret / device-local namespace is never written
        // to the bundle (DL-24). This is the single chokepoint the guard test
        // pins.
        if is_local_only_namespace(&namespace) {
            continue;
        }
        dst.execute(
            "INSERT INTO kv
                 (namespace, key, value, content_type, logical_version, updated_at, tombstone)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                namespace,
                key,
                value,
                content_type,
                logical_version,
                updated_at,
                tombstone
            ],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `oplog` ordered by `(lamport, op_id)` — the spec's deterministic replay
/// order — preserving every column.
pub(super) fn copy_oplog(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT op_id, actor_id, workspace_id, lamport, kind, payload, created_at
               FROM oplog ORDER BY lamport, op_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (op_id, actor_id, workspace_id, lamport, kind, payload, created_at) =
            r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO oplog
                 (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                op_id,
                actor_id,
                workspace_id,
                lamport,
                kind,
                payload,
                created_at
            ],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `crdt_chunks` — the append-only CRDT source of truth (DL-6) — ordered by
/// `(doc_id, created_at, chunk_id)`, preserving `created_at` so the import's
/// chunk replay order matches the source exactly.
pub(super) fn copy_crdt_chunks(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT doc_id, chunk_id, format, payload, created_at
               FROM crdt_chunks ORDER BY doc_id, created_at, chunk_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (doc_id, chunk_id, format, payload, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc_id, chunk_id, format, payload, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `crdt_snapshots` (the snapshot accelerator) ordered by `(doc_id,
/// created_at, snapshot_id)`. Snapshots are not the sole source of truth, but
/// they round-trip so the bundle is a complete workspace.
pub(super) fn copy_crdt_snapshots(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT doc_id, snapshot_id, format, payload, frontier, created_at
               FROM crdt_snapshots ORDER BY doc_id, created_at, snapshot_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (doc_id, snapshot_id, format, payload, frontier, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO crdt_snapshots
                 (doc_id, snapshot_id, format, payload, frontier, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![doc_id, snapshot_id, format, payload, frontier, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy the `records` projection ordered by `(collection, id)`. Used only by the
/// export path (for an inspectable, byte-stable bundle); import re-derives this
/// table from `crdt_chunks` so it is never trusted as input.
pub(super) fn copy_records(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare("SELECT collection, id, data, updated_at FROM records ORDER BY collection, id")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (collection, id, data, updated_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO records (collection, id, data, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![collection, id, data, updated_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `runs` ordered by `(created_at, run_id)` (policy-gated). Each row's
/// `record_json` is the full `RunRecord`; the validate-on-read in
/// [`Store::load_run`](crate::Store::load_run) still guards provenance after import.
pub(super) fn copy_runs(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT run_id, applet_id, record_json, created_at FROM runs ORDER BY created_at, run_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (run_id, applet_id, record_json, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO runs (run_id, applet_id, record_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, applet_id, record_json, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `run_logs` ordered by `(run_id, seq)` (policy-gated), preserving log
/// sequence order.
pub(super) fn copy_run_logs(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT run_id, seq, level, event_type, payload, created_at
               FROM run_logs ORDER BY run_id, seq",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (run_id, seq, level, event_type, payload, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![run_id, seq, level, event_type, payload, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}
