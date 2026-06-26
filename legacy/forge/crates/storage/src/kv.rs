//! KV namespace accessors (`ctx.storage`, DL-18) and the atomic decimal counter
//! the core uses to mint unique per-execution `run_id`s.

use forge_domain::{CoreError, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::errors::{map_sql, parse_counter_value, CounterError};
use crate::store::{now_ms, Store};

/// Max attempts for an atomic counter reservation that hits `SQLITE_BUSY` even
/// after the busy-timeout window (review 038 finding 3). Each retry re-runs the
/// whole `BEGIN IMMEDIATE` reservation, so the loser of a race observes the
/// winner's committed value rather than surfacing `database is locked`.
const COUNTER_BUSY_RETRIES: u32 = 8;

impl Store {
    // --- KV (ctx.storage namespaces, DL-18) ------------------------------

    /// Read a live (non-tombstoned) KV value.
    pub fn kv_get(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>> {
        self.conn
            .query_row(
                "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2 AND tombstone = 0",
                params![namespace, key],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map_err(map_sql)
            .map(Option::flatten)
    }

    /// Upsert a KV value, clearing any prior tombstone and bumping the logical
    /// version. Delegates to [`kv_set_tx`] inside a single transaction so this
    /// stand-alone write and the transaction-scoped form share one SQL seam.
    pub fn kv_set(
        &mut self,
        namespace: &str,
        key: &str,
        value: &[u8],
        content_type: &str,
    ) -> Result<()> {
        self.transact(|tx| kv_set_tx(tx, namespace, key, value, content_type))
    }

    /// Soft-delete a KV value (tombstone). The row is retained so the delete is
    /// sync-correct and `logical_version` keeps advancing. Delegates to
    /// [`kv_delete_tx`] so the stand-alone and transaction-scoped forms share one
    /// SQL seam.
    pub fn kv_delete(&mut self, namespace: &str, key: &str) -> Result<()> {
        self.transact(|tx| kv_delete_tx(tx, namespace, key))
    }

    /// Atomically increment a persisted decimal counter stored at `(namespace,
    /// key)` in the KV table, returning the value assigned to this reservation
    /// (the first reservation yields `1`).
    ///
    /// The read-bump-write runs inside a single **`BEGIN IMMEDIATE`** SQLite
    /// transaction, so the writer lock is taken at `BEGIN` — before the `SELECT` —
    /// rather than being upgraded lazily at the first write. With the default
    /// `DEFERRED` transaction two file-backed handles could both `SELECT` the same
    /// snapshot and then race to upgrade to a write lock; the loser surfaced
    /// `database is locked`/`StorageError` (or, worse on some builds, overwrote the
    /// winner) instead of observing the committed value (review 038 finding 3).
    /// `IMMEDIATE` serializes the two reservations: the second transaction cannot
    /// even read until the first commits, so it reads the first's value and bumps
    /// past it. The `busy_timeout` set on open blocks-then-acquires when the lock
    /// is held; the [`COUNTER_BUSY_RETRIES`] loop below re-runs the whole
    /// reservation in the rare event the timeout still elapses under `SQLITE_BUSY`.
    ///
    /// This is the atomic primitive the core uses to mint a unique per-execution
    /// `run_id` even when two `WorkspaceCore` handles share the same file (review
    /// 036/038): without it a separate read-then-write could hand the same number
    /// to two runs, and the `run_id`-keyed `save_run` would silently overwrite one
    /// audit record.
    ///
    /// The value column stores the counter as utf-8 decimal text (matching
    /// [`kv_set`](Self::kv_set)'s `text/plain` content type); a non-utf-8 or
    /// non-integer existing value is a `StorageError` rather than a silent reset.
    pub fn next_counter(&mut self, namespace: &str, key: &str) -> Result<u64> {
        let mut attempt = 0u32;
        loop {
            match self.next_counter_immediate(namespace, key) {
                Ok(next) => return Ok(next),
                // Transient lock contention that outlasted the busy-timeout: the
                // whole IMMEDIATE reservation only commits on success, so re-run
                // it. The loser thus observes the winner's committed value rather
                // than failing with `database is locked`.
                Err(CounterError::Busy(_)) if attempt < COUNTER_BUSY_RETRIES => {
                    attempt += 1;
                }
                Err(CounterError::Busy(e)) => return Err(map_sql(e)),
                Err(CounterError::Fatal(err)) => return Err(err),
            }
        }
    }

    /// One `BEGIN IMMEDIATE` counter reservation (see [`next_counter`](Self::next_counter)).
    /// Distinguishes a transient `SQLITE_BUSY` (retryable) from a permanent error
    /// (surface). Any error rolls the transaction back.
    fn next_counter_immediate(
        &mut self,
        namespace: &str,
        key: &str,
    ) -> std::result::Result<u64, CounterError> {
        // Take the writer lock at BEGIN, not lazily at first write: with IMMEDIATE
        // a second contending handle blocks here (busy_timeout) instead of reading
        // a stale snapshot it could later fail to upgrade.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CounterError::from_sql)?;
        let result = (|| -> Result<u64> {
            let current: u64 = tx
                .query_row(
                    "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2 AND tombstone = 0",
                    params![namespace, key],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()
                .map_err(map_sql)?
                .flatten()
                .map(|bytes| parse_counter_value(&bytes))
                .transpose()?
                .unwrap_or(0);
            let next = current
                .checked_add(1)
                .ok_or_else(|| CoreError::StorageError("run counter overflowed u64".into()))?;
            tx.execute(
                "INSERT INTO kv
                     (namespace, key, value, content_type, logical_version, updated_at, tombstone)
                 VALUES (?1, ?2, ?3, 'text/plain', 1, ?4, 0)
                 ON CONFLICT(namespace, key) DO UPDATE SET
                     value = excluded.value,
                     content_type = excluded.content_type,
                     logical_version = kv.logical_version + 1,
                     updated_at = excluded.updated_at,
                     tombstone = 0",
                params![namespace, key, next.to_string().as_bytes(), now_ms()],
            )
            .map_err(map_sql)?;
            Ok(next)
        })();
        match result {
            Ok(next) => tx.commit().map(|()| next).map_err(CounterError::from_sql),
            Err(err) => {
                // Dropping the tx rolls back; rollback explicitly to surface any
                // rollback failure. The inner body's errors are already mapped
                // CoreErrors (with the lock taken at BEGIN, the SELECT/write never
                // hit SQLITE_BUSY), so they are always fatal here.
                let _ = tx.rollback();
                Err(CounterError::Fatal(err))
            }
        }
    }

    /// List live keys in `namespace` whose key starts with `prefix`, sorted.
    /// Tombstoned entries are skipped.
    pub fn kv_list(&self, namespace: &str, prefix: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT key FROM kv
                  WHERE namespace = ?1 AND tombstone = 0 AND key LIKE ?2 ESCAPE '\\'
                  ORDER BY key",
            )
            .map_err(map_sql)?;
        let like = format!("{}%", escape_like(prefix));
        let rows = stmt
            .query_map(params![namespace, like], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }
}

// --- Transaction-scoped KV helpers (for grouped atomic writes) -------------
//
// The core's lifecycle commit (CR-7 `applet.upgrade` / `applet.uninstall`)
// needs the schema-registry persist, the active-pointer switch, the program
// pin, and the tombstones to land in ONE SQLite transaction so a crash mid-way
// cannot leave the workspace half-upgraded (schema committed but active pointer
// not switched). These mirror `kv_set` / `kv_get` / `kv_delete` exactly but
// execute against an open [`rusqlite::Transaction`], so several KV writes can be
// composed inside a single `Store::transact` closure and commit-or-roll-back
// together. `kv_set` / `kv_delete` delegate to the `_tx` forms above.

/// Read a live (non-tombstoned) KV value inside an open transaction.
pub fn kv_get_tx(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    key: &str,
) -> Result<Option<Vec<u8>>> {
    tx.query_row(
        "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2 AND tombstone = 0",
        params![namespace, key],
        |row| row.get::<_, Option<Vec<u8>>>(0),
    )
    .optional()
    .map_err(map_sql)
    .map(Option::flatten)
}

/// Upsert a KV value inside an open transaction, clearing any prior tombstone
/// and bumping the logical version (the tx-scoped form of [`Store::kv_set`]).
pub fn kv_set_tx(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    key: &str,
    value: &[u8],
    content_type: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO kv
             (namespace, key, value, content_type, logical_version, updated_at, tombstone)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, 0)
         ON CONFLICT(namespace, key) DO UPDATE SET
             value = excluded.value,
             content_type = excluded.content_type,
             logical_version = kv.logical_version + 1,
             updated_at = excluded.updated_at,
             tombstone = 0",
        params![namespace, key, value, content_type, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Soft-delete a KV value (tombstone) inside an open transaction (the tx-scoped
/// form of [`Store::kv_delete`]).
pub fn kv_delete_tx(tx: &rusqlite::Transaction<'_>, namespace: &str, key: &str) -> Result<()> {
    tx.execute(
        "UPDATE kv
             SET tombstone = 1,
                 value = NULL,
                 logical_version = logical_version + 1,
                 updated_at = ?3
           WHERE namespace = ?1 AND key = ?2",
        params![namespace, key, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Escape SQLite `LIKE` metacharacters (`%`, `_`, and the `\` escape itself)
/// so a prefix is matched literally. Pairs with `ESCAPE '\'` in the query.
fn escape_like(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len());
    for c in prefix.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}
