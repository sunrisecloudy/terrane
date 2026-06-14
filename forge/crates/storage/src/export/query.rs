//! Read-only inspection of a workspace/bundle: the fresh-target check
//! ([`Store::is_empty_target`]) and the bundle `meta` reader ([`bundle_meta`]).

use super::guard::is_local_only_namespace;
use super::transaction::table_has_any_row;
use crate::{map_sql, Store};
use forge_domain::{CoreError, Result};
use rusqlite::{params, OptionalExtension};

impl Store {
    /// True iff this store holds **no importable state at all** — the precondition
    /// for an in-place import (review 062 P1 #2). Checks EVERY table/namespace a
    /// bundle would populate, so a workspace that is "empty" only in its records
    /// projection but already carries (say) a `db.read` grant table or applet
    /// manifests is correctly reported as **not** fresh:
    ///
    /// - the projected `records`,
    /// - the CRDT source of truth (`crdt_chunks`, `crdt_snapshots`) and `oplog`,
    /// - the policy-gated `runs` / `run_logs`,
    /// - and every **portable** `kv` row — i.e. any namespace NOT dropped by
    ///   [`is_local_only_namespace`](super::is_local_only_namespace). Local-only /
    ///   secret namespaces are excluded because they never travel in a bundle, so a
    ///   store holding *only* a stray device-local key is still a valid (fresh)
    ///   import target; counting them would wrongly refuse a genuinely importable
    ///   workspace.
    ///
    /// (M0a storage has no `schema_defs` / `index_defs` tables yet — those are
    /// GA-future, reported as `missing_required_for_ga` in the export descriptor —
    /// so there is nothing to check for them here; this is the complete importable
    /// surface for the current schema.)
    pub fn is_empty_target(&self) -> Result<bool> {
        // Any importable physical table with a row → not fresh. A single
        // `EXISTS`/`LIMIT 1` per table is enough (we only need presence).
        for table in [
            "records",
            "crdt_chunks",
            "crdt_snapshots",
            "oplog",
            "runs",
            "run_logs",
        ] {
            if table_has_any_row(&self.conn, table)? {
                return Ok(false);
            }
        }
        // Portable kv: a row in any namespace a bundle WOULD carry counts. A row in
        // a purely local-only / secret namespace (never exported) does not.
        if self.has_portable_kv_row()? {
            return Ok(false);
        }
        Ok(true)
    }

    /// True iff the `kv` table holds at least one row in a **portable** namespace —
    /// one a bundle would carry (i.e. NOT [`is_local_only_namespace`]). Tombstoned
    /// rows count: a deleted-but-exported key is still importable state that a fresh
    /// target must not silently shadow. Local-only / secret namespaces are skipped
    /// because they never travel in a bundle.
    fn has_portable_kv_row(&self) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT namespace FROM kv")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        for r in rows {
            let namespace = r.map_err(map_sql)?;
            if !is_local_only_namespace(&namespace) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Read a bundle's `meta` value as utf-8 text (helper for callers / tests that
/// want to inspect the header without reaching SQLite). Returns `None` for a
/// missing key. A non-utf-8 value is a `StorageError` rather than lossy bytes.
pub fn bundle_meta(bundle: &Store, key: &str) -> Result<Option<String>> {
    let raw: Option<Vec<u8>> = bundle
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten();
    match raw {
        Some(bytes) => {
            let s = std::str::from_utf8(&bytes).map_err(|e| {
                CoreError::StorageError(format!("bundle meta {key} is not utf-8: {e}"))
            })?;
            Ok(Some(s.to_string()))
        }
        None => Ok(None),
    }
}
