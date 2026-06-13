//! forge-storage: SQLite KV/oplog substrate + records projection.
//!
//! prd-merged/02-data-layer-prd.md §4 (physical layout): a `Store` wraps a
//! single `rusqlite::Connection` opened on the portable workspace file. It
//! provides the M0a physical subset of the schema and the typed accessors the
//! rest of the spine needs:
//!
//! - **KV** (`kv` table) — per-applet `ctx.storage` namespaces (DL-18).
//! - **Records projection** (`records` table, canonical JSON `TEXT` via JSON1,
//!   DL-4) — what `ctx.db` reads/writes and the projection materializes.
//! - **Oplog** (`oplog`) and **CRDT blobs** (`crdt_chunks`/`crdt_snapshots`) —
//!   the append-only substrate the `crdt` crate folds into the projection
//!   (DL-4 single-transaction writes, DL-6 rebuild source of truth).
//! - **Runs** (`runs`) — the full `RunRecord` JSON that `runtime.replay` reads.
//!
//! Durability follows DL-23: `journal_mode=WAL`, `synchronous=NORMAL`. Every
//! fallible call maps `rusqlite::Error` to [`CoreError::StorageError`]; the
//! connection path never `unwrap`s on external input.

use forge_domain::{CoreError, RecordEnvelope, Result, RunRecord};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod query;

pub use query::{
    compile_select, AggregateResult, CompiledSelect, Dir, FieldRef, Filter, FullScanReason,
    GroupResult, Mutation, Op, OrderBy, PlannedQuery, PlannerWarning, Predicate, Query, QueryResult,
    QueryRow, TextSearch,
};

pub mod index;
pub use index::{CreateIndexKind, IndexDef, IndexKind, IndexManager, IndexState};

pub mod crdt_write;
pub use crdt_write::{collection_doc_id, CHUNK_FORMAT, LOCAL_PEER_ID};

pub mod export;
pub use export::{
    bundle_meta, is_local_only_namespace, ExportOptions, RunLogPolicy, EXPORT_FORMAT_VERSION,
    STORAGE_SCHEMA_VERSION,
};

/// The M0a physical schema (prd-merged/02 §4 subset). Created on open if
/// absent. Tables that exist in the full spec but are unused by the spine are
/// deliberately omitted; the columns present match the spec names so the file
/// stays inspectable and forward-compatible.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key        TEXT PRIMARY KEY,
    value      BLOB,
    updated_at INTEGER
);

CREATE TABLE IF NOT EXISTS kv (
    namespace       TEXT NOT NULL,
    key             TEXT NOT NULL,
    value           BLOB,
    content_type    TEXT,
    logical_version INTEGER,
    updated_at      INTEGER,
    tombstone       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (namespace, key)
);

CREATE TABLE IF NOT EXISTS oplog (
    op_id        TEXT PRIMARY KEY,
    actor_id     TEXT,
    workspace_id TEXT,
    lamport      INTEGER,
    kind         TEXT,
    payload      BLOB,
    created_at   INTEGER
);

CREATE TABLE IF NOT EXISTS crdt_chunks (
    doc_id     TEXT NOT NULL,
    chunk_id   TEXT NOT NULL,
    format     TEXT,
    payload    BLOB,
    created_at INTEGER,
    PRIMARY KEY (doc_id, chunk_id)
);

CREATE TABLE IF NOT EXISTS crdt_snapshots (
    doc_id      TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    format      TEXT,
    payload     BLOB,
    frontier    BLOB,
    created_at  INTEGER,
    PRIMARY KEY (doc_id, snapshot_id)
);

CREATE TABLE IF NOT EXISTS records (
    collection TEXT NOT NULL,
    id         TEXT NOT NULL,
    data       TEXT NOT NULL,
    updated_at INTEGER,
    PRIMARY KEY (collection, id)
);

CREATE TABLE IF NOT EXISTS run_logs (
    run_id     TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    level      TEXT,
    event_type TEXT,
    payload    BLOB,
    created_at INTEGER,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS runs (
    run_id     TEXT PRIMARY KEY,
    applet_id  TEXT,
    record_json TEXT NOT NULL,
    created_at INTEGER
);
"#;

/// How long a contended write waits for the SQLite writer lock before giving up
/// (review 038 finding 3). Bounds the block when two file-backed handles contend.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Max attempts for an atomic counter reservation that hits `SQLITE_BUSY` even
/// after the busy-timeout window (review 038 finding 3). Each retry re-runs the
/// whole `BEGIN IMMEDIATE` reservation, so the loser of a race observes the
/// winner's committed value rather than surfacing `database is locked`.
const COUNTER_BUSY_RETRIES: u32 = 8;

/// Map any `rusqlite` failure to a stable, displayable `CoreError`.
fn map_sql(e: rusqlite::Error) -> CoreError {
    CoreError::StorageError(e.to_string())
}

/// True iff a `rusqlite` error is a transient SQLite lock contention
/// (`SQLITE_BUSY` / `SQLITE_LOCKED`), which a serialized retry can resolve — as
/// opposed to a permanent failure (corruption, constraint, misuse) that must
/// surface. Used by [`Store::next_counter`]'s bounded retry loop.
fn is_busy(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(err.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Outcome of one `BEGIN IMMEDIATE` counter reservation: either a retryable
/// lock-contention (`Busy`, carrying the raw error so the caller can surface it
/// after exhausting retries) or a permanent failure (`Fatal`, already mapped).
enum CounterError {
    Busy(rusqlite::Error),
    Fatal(CoreError),
}

impl CounterError {
    /// Classify a raw `rusqlite` error from the BEGIN/commit boundary: a
    /// `SQLITE_BUSY`/`SQLITE_LOCKED` is retryable, everything else is fatal.
    fn from_sql(e: rusqlite::Error) -> Self {
        if is_busy(&e) {
            CounterError::Busy(e)
        } else {
            CounterError::Fatal(map_sql(e))
        }
    }
}

/// Map a serde_json (de)serialization failure on the storage path.
fn map_json(ctx: &str, e: serde_json::Error) -> CoreError {
    CoreError::StorageError(format!("{ctx}: {e}"))
}

/// Parse a persisted counter value (utf-8 decimal `u64`) for
/// [`Store::next_counter`], surfacing a `StorageError` on corruption rather than
/// silently resetting to zero.
fn parse_counter_value(bytes: &[u8]) -> Result<u64> {
    let s = std::str::from_utf8(bytes)
        .map_err(|e| CoreError::StorageError(format!("counter value is not utf-8: {e}")))?;
    s.parse::<u64>()
        .map_err(|e| CoreError::StorageError(format!("counter value is malformed: {e}")))
}

/// Wall-clock milliseconds since the Unix epoch, used for the `updated_at` /
/// `created_at` substrate columns. This is metadata only; logical ordering for
/// the deterministic spine lives in `LogicalTimestamp`/`lamport`, not here. A
/// clock before the epoch (impossible in practice) degrades to `0` rather than
/// panicking.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A handle to the workspace SQLite store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if absent) a file-backed workspace store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(map_sql)?;
        Self::init(conn)
    }

    /// Open a private in-memory store (tests, scratch, replay sandboxes).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sql)?;
        Self::init(conn)
    }

    /// Apply durability PRAGMAs (DL-23) and ensure the M0a schema exists.
    fn init(conn: Connection) -> Result<Self> {
        // journal_mode is a query pragma (returns the resulting mode); the
        // others are simple sets. `synchronous=NORMAL` + WAL is the DL-23
        // durability point.
        conn.pragma_update(None, "journal_mode", "WAL").map_err(map_sql)?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(map_sql)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(map_sql)?;
        // Two file-backed handles can contend for the single SQLite writer lock
        // (e.g. two `WorkspaceCore` instances minting run ids concurrently). Wait
        // for the lock for a bounded window instead of instantly surfacing
        // `database is locked`, so a contended `BEGIN IMMEDIATE` blocks-then-wins
        // rather than failing (review 038 finding 3). `next_counter` also retries
        // on `SQLITE_BUSY` for the (rare) case the timeout still elapses.
        conn.busy_timeout(BUSY_TIMEOUT).map_err(map_sql)?;
        conn.execute_batch(SCHEMA).map_err(map_sql)?;
        Ok(Store { conn })
    }

    /// Borrow the underlying connection (advanced/raw use, e.g. the rebuild
    /// path in the `crdt` crate). Most callers use the typed methods.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Run `f` inside a single SQLite transaction (DL-4). The transaction is
    /// committed iff `f` returns `Ok`; any `Err` rolls back, leaving the DB
    /// byte-for-byte as it was before the call.
    pub fn transact<T, F>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    {
        let tx = self.conn.transaction().map_err(map_sql)?;
        match f(&tx) {
            Ok(value) => {
                tx.commit().map_err(map_sql)?;
                Ok(value)
            }
            Err(e) => {
                // Dropping the tx rolls back; do it explicitly for clarity and
                // to surface any rollback failure.
                tx.rollback().map_err(map_sql)?;
                Err(e)
            }
        }
    }

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
    /// version.
    pub fn kv_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
        content_type: &str,
    ) -> Result<()> {
        self.conn
            .execute(
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

    /// Soft-delete a KV value (tombstone). The row is retained so the delete is
    /// sync-correct and `logical_version` keeps advancing.
    pub fn kv_delete(&self, namespace: &str, key: &str) -> Result<()> {
        self.conn
            .execute(
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

    // --- Records projection (DL-4) ---------------------------------------

    /// Materialize/overwrite a record in the projection. The full envelope is
    /// stored as canonical JSON `TEXT` in `records.data` (queryable via JSON1),
    /// and `collection`/`id` are kept as columns for the PK and lookups.
    pub fn put_record(&self, env: &RecordEnvelope) -> Result<()> {
        let data = serde_json::to_string(env).map_err(|e| map_json("put_record", e))?;
        self.conn
            .execute(
                "INSERT INTO records (collection, id, data, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(collection, id) DO UPDATE SET
                     data = excluded.data,
                     updated_at = excluded.updated_at",
                params![
                    env.collection.as_str(),
                    env.entity_id.as_str(),
                    data,
                    now_ms()
                ],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Read a single record back as a reconstructed envelope.
    pub fn get_record(&self, collection: &str, id: &str) -> Result<Option<RecordEnvelope>> {
        let data: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM records WHERE collection = ?1 AND id = ?2",
                params![collection, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        match data {
            Some(json) => {
                let env = serde_json::from_str(&json).map_err(|e| map_json("get_record", e))?;
                Ok(Some(env))
            }
            None => Ok(None),
        }
    }

    /// List every record in `collection`, ordered by id, as envelopes.
    pub fn list_records(&self, collection: &str) -> Result<Vec<RecordEnvelope>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM records WHERE collection = ?1 ORDER BY id")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map(params![collection], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let json = r.map_err(map_sql)?;
            out.push(serde_json::from_str(&json).map_err(|e| map_json("list_records", e))?);
        }
        Ok(out)
    }

    // --- Query engine (DL-15/16) -----------------------------------------

    /// Run a compiled [`Query`] against the `records` projection (DL-15).
    ///
    /// The AST is compiled to a **parameterized** SELECT over JSON1
    /// (`json_extract`); record values are bound, never interpolated (DL-16, no
    /// raw-SQL surface). Filtering happens in SQL; ordering, limit/offset, and
    /// aggregation are finalized in Rust so the spec's platform-stable total
    /// order and null-handling rules hold exactly (`query-dsl.md` §Result).
    ///
    /// Returns rows, a single aggregate, or grouped aggregates depending on the
    /// query shape.
    pub fn query(&self, q: &Query) -> Result<QueryResult> {
        // Unsupported P1 features (a bare `text`/`join` marker) must be refused
        // BEFORE planning: scanning anyway would silently return bogus rows
        // (e.g. a `join` predicate over `assignee.name` compiles to a literal
        // `$.fields."assignee.name"` path and matches nothing/garbage). Surface
        // the typed `unsupported_feature` error so the caller sees the contract,
        // not a wrong answer (review 040 finding 7; query-dsl.md §Result).
        if let Some(feature) = &q.unsupported {
            return Err(CoreError::QueryError(format!(
                "unsupported_feature: '{feature}' is not supported in M0a (P1)"
            )));
        }
        let matched = self.scan_matched(q)?;

        // Group-by: bucket by the (display) group field, then aggregate each
        // bucket. Group keys are emitted in ascending spec order.
        if let Some(group_field) = &q.group_by {
            let agg = q.aggregate.clone().unwrap_or(query::Aggregate {
                count: true,
                sum: None,
                avg: None,
                min: None,
                max: None,
            });
            let mut buckets: Vec<(serde_json::Value, Vec<&RecordEnvelope>)> = Vec::new();
            for env in &matched {
                let key = query::group_key(env, group_field);
                match buckets.iter_mut().find(|(k, _)| k == &key) {
                    Some((_, v)) => v.push(env),
                    None => buckets.push((key, vec![env])),
                }
            }
            buckets.sort_by(|a, b| query::cmp_json_pub(&a.0, &b.0));
            let groups = buckets
                .into_iter()
                .map(|(key, rows)| GroupResult {
                    key,
                    aggregate: query::compute_aggregate(&rows, &agg),
                })
                .collect();
            return Ok(QueryResult::Groups(groups));
        }

        // Bare aggregate over the matched set.
        if let Some(agg) = &q.aggregate {
            let refs: Vec<&RecordEnvelope> = matched.iter().collect();
            return Ok(QueryResult::Aggregate(query::compute_aggregate(&refs, agg)));
        }

        // Row result: wrap, then order/offset/limit in Rust.
        let rows: Vec<QueryRow> = matched
            .into_iter()
            .map(|env| QueryRow {
                id: env.entity_id.as_str().to_string(),
                envelope: env,
            })
            .collect();
        Ok(QueryResult::Rows(query::finalize_rows(rows, q)))
    }

    /// Execute the compiled filter and return the matched envelopes (unordered).
    /// Shared by the row, aggregate, and group-by paths.
    fn scan_matched(&self, q: &Query) -> Result<Vec<RecordEnvelope>> {
        let compiled = compile_select(q)?;
        let mut stmt = self.conn.prepare(&compiled.sql).map_err(map_sql)?;
        let bound = to_sql_params(&compiled.params)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get::<_, String>(1))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let json = r.map_err(map_sql)?;
            out.push(serde_json::from_str(&json).map_err(|e| map_json("query", e))?);
        }
        Ok(out)
    }

    /// Count live records in `collection` (the planner's `estimated_rows`).
    fn count_records(&self, collection: &str, include_deleted: bool) -> Result<i64> {
        let sql = if include_deleted {
            "SELECT COUNT(*) FROM records WHERE collection = ?1"
        } else {
            "SELECT COUNT(*) FROM records WHERE collection = ?1 \
             AND json_extract(data, '$.deleted') IS NOT 1"
        };
        self.conn
            .query_row(sql, params![collection], |r| r.get::<_, i64>(0))
            .map_err(map_sql)
    }

    // --- Index-aware planner (DL-5/DL-6) ---------------------------------

    /// Run a [`Query`] with index awareness against `indexes` (DL-5/DL-6).
    ///
    /// Returns the same rows/aggregates as [`query`](Self::query) — `records` is
    /// canonical, so the answer never depends on whether an index exists — plus
    /// the planner decision: `uses_index`, the `index_id` used, and any
    /// `planner.full_scan` warnings. The index decision is computed from the
    /// registered definitions and their lifecycle states (never hardcoded): an
    /// active expression index serves eq/range/order over its stable field id; an
    /// active FTS5 shadow table serves a text search; every other case scans and
    /// warns.
    ///
    /// A text search is not a bypass: the FTS shadow table (or a portable
    /// fallback scan) produces a MATCH set in rank order, then the same
    /// `filter`/`group`/`aggregate`/`order`/`limit`/`offset` pipeline is applied
    /// to that set as for a scalar query (DL-15; review 041/042 finding 4). FTS
    /// rank order is preserved unless an explicit non-rank `order_by` overrides it.
    pub fn query_planned(
        &self,
        q: &Query,
        indexes: &index::IndexManager,
    ) -> Result<query::PlannedQuery> {
        // Same guard as `query`: refuse an unsupported P1 feature before planning
        // so we never plan/scan a query that would return bogus rows (review 040
        // finding 7).
        if let Some(feature) = &q.unsupported {
            return Err(CoreError::QueryError(format!(
                "unsupported_feature: '{feature}' is not supported in M0a (P1)"
            )));
        }
        let estimated = self.count_records(&q.from, q.include_deleted)?;
        let plan = indexes.plan(q, estimated);

        // Text-search path: rows come from the FTS table when it is active,
        // otherwise from a portable `like`-style scan over the records.
        if let Some(ts) = &q.text_search {
            let result = self.run_text_search(q, ts, &plan, indexes)?;
            return Ok(query::PlannedQuery {
                result,
                uses_index: plan.uses_index,
                index_id: plan.index_id,
                warnings: plan.warnings,
            });
        }

        // Scalar path: identical to `query`, with the planner decision attached.
        let result = self.query(q)?;
        Ok(query::PlannedQuery {
            result,
            uses_index: plan.uses_index,
            index_id: plan.index_id,
            warnings: plan.warnings,
        })
    }

    /// Resolve a text search as a **MATCH source inside the normal query
    /// pipeline** (DL-15). The FTS5 shadow table (or a portable fallback scan)
    /// produces the candidate id set in rank order; the rest of the query —
    /// `filter`, `group_by`, `aggregate`, `order_by`/`limit`/`offset` — is then
    /// applied to exactly that set, just like a non-text query (review 041/042
    /// finding 4). FTS rank ordering is preserved when the query requests it (or
    /// leaves the order default); an explicit non-rank `order_by` wins.
    fn run_text_search(
        &self,
        q: &Query,
        ts: &query::TextSearch,
        plan: &index::IndexPlan,
        indexes: &index::IndexManager,
    ) -> Result<QueryResult> {
        // 1. The MATCH set: candidate ids in FTS rank order (or fallback scan).
        let match_ids: Vec<String> = if plan.uses_index {
            let field_id = ts
                .field
                .field_id()
                .ok_or_else(|| CoreError::QueryError("text search needs a stable field id".into()))?;
            indexes.fts_match(&self.conn, &q.from, field_id, &ts.query)?
        } else {
            self.text_search_scan(q, ts)?
        };
        // rank position by id (FTS already ordered by rank; index = rank).
        let rank_of: std::collections::HashMap<&str, usize> = match_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();

        // 2. Apply the query's filter over canonical records (same SQL semantics
        //    as a non-text query), then intersect with the MATCH set so the text
        //    search composes with `where`. Records are canonical, so this is the
        //    correct row set regardless of the FTS path.
        let matched: Vec<RecordEnvelope> = self
            .scan_matched(q)?
            .into_iter()
            .filter(|env| rank_of.contains_key(env.entity_id.as_str()))
            .collect();

        // 3. Group-by / aggregate over the composed set (identical to `query`).
        if let Some(group_field) = &q.group_by {
            let agg = q.aggregate.clone().unwrap_or(query::Aggregate {
                count: true,
                sum: None,
                avg: None,
                min: None,
                max: None,
            });
            let mut buckets: Vec<(serde_json::Value, Vec<&RecordEnvelope>)> = Vec::new();
            for env in &matched {
                let key = query::group_key(env, group_field);
                match buckets.iter_mut().find(|(k, _)| k == &key) {
                    Some((_, v)) => v.push(env),
                    None => buckets.push((key, vec![env])),
                }
            }
            buckets.sort_by(|a, b| query::cmp_json_pub(&a.0, &b.0));
            let groups = buckets
                .into_iter()
                .map(|(key, rows)| GroupResult {
                    key,
                    aggregate: query::compute_aggregate(&rows, &agg),
                })
                .collect();
            return Ok(QueryResult::Groups(groups));
        }
        if let Some(agg) = &q.aggregate {
            let refs: Vec<&RecordEnvelope> = matched.iter().collect();
            return Ok(QueryResult::Aggregate(query::compute_aggregate(&refs, agg)));
        }

        // 4. Row result. An explicit non-rank `order_by` is finalized with the
        //    spec total order (and its limit/offset). Otherwise FTS rank order is
        //    preserved, and the rank path's limit/offset are applied here (the
        //    bug review 041/042 finding 4 calls out: rank-path limit/offset were
        //    previously dropped).
        let rows: Vec<QueryRow> = matched
            .into_iter()
            .map(|env| QueryRow {
                id: env.entity_id.as_str().to_string(),
                envelope: env,
            })
            .collect();
        let rank_order = q
            .order_by
            .as_ref()
            .map(|ob| matches!(&ob.field, query::FieldRef::Name(n) if n == "rank"))
            .unwrap_or(true);
        let rows = if rank_order {
            self.finalize_rank_ordered(rows, &rank_of, q)
        } else {
            query::finalize_rows(rows, q)
        };
        Ok(QueryResult::Rows(rows))
    }

    /// Order a text-search row set by FTS rank (rank position, then entity id as
    /// a stable tie-break), then apply the query's `offset`/`limit`. Used when
    /// the query keeps FTS rank order (default or explicit `rank`); the rank-path
    /// limit/offset are applied here so they are not silently dropped.
    fn finalize_rank_ordered(
        &self,
        mut rows: Vec<QueryRow>,
        rank_of: &std::collections::HashMap<&str, usize>,
        q: &Query,
    ) -> Vec<QueryRow> {
        rows.sort_by(|a, b| {
            let ra = rank_of.get(a.id.as_str()).copied().unwrap_or(usize::MAX);
            let rb = rank_of.get(b.id.as_str()).copied().unwrap_or(usize::MAX);
            ra.cmp(&rb).then_with(|| a.id.cmp(&b.id))
        });
        if let Some(off) = q.offset {
            let off = off as usize;
            if off >= rows.len() {
                rows.clear();
            } else {
                rows.drain(0..off);
            }
        }
        if let Some(lim) = q.limit {
            rows.truncate(lim as usize);
        }
        rows
    }

    /// Portable text-search fallback: ASCII case-insensitive substring match over
    /// the field's stored value. Used when no active FTS table covers the search,
    /// so the rows are still correct (records are canonical) while the planner
    /// surfaces the `fts_not_available` warning.
    fn text_search_scan(&self, q: &Query, ts: &query::TextSearch) -> Result<Vec<String>> {
        // Use the same double-quoted JSON path the planner/index DDL emit, so a
        // dotted field id resolves to the literal key (not a nested path).
        let path = match &ts.field {
            query::FieldRef::Id(id) => query::field_id_json_path(id),
            query::FieldRef::Name(n) => format!("$.fields.{}", query::quote_json_path_key(n)),
        };
        let sql = "SELECT id, json_extract(data, ?1) FROM records \
                   WHERE collection = ?2 AND json_extract(data, '$.deleted') IS NOT 1";
        let mut stmt = self.conn.prepare(sql).map_err(map_sql)?;
        let needle = ts.query.to_ascii_lowercase();
        let rows = stmt
            .query_map(params![path, q.from], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, value) = r.map_err(map_sql)?;
            if let Some(text) = value {
                if text.to_ascii_lowercase().contains(&needle) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Create the physical structures for every active index in `indexes`
    /// (idempotent expression-index DDL + populated FTS5 shadow tables), built
    /// from canonical `records`. Thin wrapper over
    /// [`IndexManager::rebuild_active`](index::IndexManager::rebuild_active) so
    /// callers need not reach the connection.
    pub fn build_indexes(&self, indexes: &index::IndexManager) -> Result<()> {
        indexes.rebuild_active(&self.conn)
    }

    /// Create (DL-5) one index over the `records` projection and build it from
    /// canonical records in a single call: `Value` → a collection-scoped JSON1
    /// expression index, `Fts` → a populated FTS5 shadow table. The definition is
    /// registered `Active` in `indexes` and its physical structure is built
    /// immediately (so creating an index *after* rows exist activates it — DL-6).
    /// Returns the deterministic `index_id`. Thin wrapper over
    /// [`IndexManager::create_index`](index::IndexManager::create_index).
    pub fn create_index(
        &self,
        indexes: &mut index::IndexManager,
        collection: &str,
        field_id: &str,
        kind: index::CreateIndexKind,
    ) -> Result<String> {
        indexes.create_index(&self.conn, collection, field_id, kind)
    }

    // --- Index-synced record writes (DL-5 FTS maintenance) ----------------

    /// Put a record (as [`put_record`](Self::put_record)) **and** refresh any
    /// active FTS5 shadow rows for it in the **same** SQLite transaction (DL-5:
    /// FTS must be kept in sync on insert/update). Expression indexes are
    /// maintained by SQLite automatically, so only FTS needs the hand-sync; the
    /// canonical `records` write and the FTS refresh commit or roll back together.
    pub fn put_record_indexed(
        &mut self,
        env: &RecordEnvelope,
        indexes: &index::IndexManager,
    ) -> Result<()> {
        let data = serde_json::to_string(env).map_err(|e| map_json("put_record_indexed", e))?;
        let collection = env.collection.as_str().to_string();
        let id = env.entity_id.as_str().to_string();
        self.transact(|tx| {
            put_record_tx(tx, env)?;
            indexes.sync_fts_for_record(tx, &collection, &id, &data)
        })
    }

    /// Patch a record (as [`patch_record`](Self::patch_record)) and refresh active
    /// FTS rows for it in the same transaction (DL-5). Returns the merged
    /// envelope.
    pub fn patch_record_indexed(
        &mut self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
        indexes: &index::IndexManager,
    ) -> Result<RecordEnvelope> {
        self.transact(|tx| {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
            })?;
            for (k, v) in fields {
                env.fields.insert(k.clone(), v.clone());
            }
            bump_updated_at(&mut env, logical_at);
            put_record_tx(tx, &env)?;
            let data =
                serde_json::to_string(&env).map_err(|e| map_json("patch_record_indexed", e))?;
            indexes.sync_fts_for_record(tx, collection, id, &data)?;
            Ok(env)
        })
    }

    /// Delete (tombstone) a record (as [`delete_record`](Self::delete_record)) and
    /// drop it from any active FTS shadow rows in the same transaction (DL-5): a
    /// deleted record stops matching text search. Returns the tombstoned envelope.
    pub fn delete_record_indexed(
        &mut self,
        collection: &str,
        id: &str,
        logical_at: Option<i64>,
        indexes: &index::IndexManager,
    ) -> Result<RecordEnvelope> {
        self.transact(|tx| {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
            })?;
            env.deleted = true;
            bump_updated_at(&mut env, logical_at);
            put_record_tx(tx, &env)?;
            let data =
                serde_json::to_string(&env).map_err(|e| map_json("delete_record_indexed", e))?;
            indexes.sync_fts_for_record(tx, collection, id, &data)?;
            Ok(env)
        })
    }

    // --- Mutations (DL-17) -----------------------------------------------

    /// Replace a record's known display fields (DL-17 `update`). Fields the
    /// caller does not mention are dropped from `fields`, but `field_ids`,
    /// `unknown_fields`, and `extensions` are preserved (DL-9). A missing record
    /// is a `QueryError`. `logical_at`, when given, advances `updated_at`.
    pub fn update_record(
        &self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("update: record {collection}/{id} does not exist"))
        })?;
        env.fields = fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Merge the supplied fields into a record (DL-17 `patch`), preserving fields
    /// the caller omits. A missing record is a `QueryError`. `logical_at`, when
    /// given, advances `updated_at`.
    pub fn patch_record(
        &self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
        })?;
        for (k, v) in fields {
            env.fields.insert(k.clone(), v.clone());
        }
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Tombstone a record (DL-17 `delete`, DL-21 sync-correct soft delete). The
    /// row is retained with `deleted = true` so the delete syncs; query hides it
    /// unless `includeDeleted`. A missing record is a `QueryError`.
    pub fn delete_record(
        &self,
        collection: &str,
        id: &str,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
        })?;
        env.deleted = true;
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Apply a single [`Mutation`] outside a group (its own statement), keeping
    /// active FTS5 shadow tables in sync in the **same** transaction (DL-5/DL-17).
    ///
    /// This is the applet-facing DL-17 mutation surface, so it must not bypass
    /// dynamic-index maintenance: insert/update/patch/delete each refresh any
    /// active FTS rows for the touched record atomically with the projection
    /// write (review 041/042 finding 3). A nested `transact` here is rejected —
    /// use [`transact_mutations`](Self::transact_mutations) for groups.
    ///
    /// Pass the workspace's [`IndexManager`](index::IndexManager); when no FTS
    /// index is active the sync is a cheap no-op, but it can never be skipped by
    /// going through this surface (the unsynced [`put_record`](Self::put_record)
    /// family is reserved for projection rebuild, not applet writes).
    pub fn apply_mutation(
        &mut self,
        m: &Mutation,
        indexes: &index::IndexManager,
    ) -> Result<()> {
        if matches!(m, Mutation::Transact { .. }) {
            return Err(CoreError::QueryError(
                "nested transact is not allowed; pass items to transact_mutations".into(),
            ));
        }
        self.transact(|tx| {
            apply_mutation_tx(tx, m, indexes)?;
            Ok(())
        })
    }

    /// Apply a group of mutations as one local SQLite transaction (DL-17
    /// `transact`): all-or-nothing. A failure rolls back the whole group, so the
    /// projection is left byte-for-byte unchanged (reuses [`transact`](Self::transact)).
    ///
    /// Active FTS5 shadow tables are refreshed for every touched record inside
    /// the same transaction (DL-5), so a record inserted/patched here is
    /// immediately searchable without a manual rebuild (review 041/042 finding 3).
    ///
    /// Returns the number of leaf mutations applied. Items may themselves be a
    /// `transact` group; nested items are flattened into the same transaction.
    pub fn transact_mutations(
        &mut self,
        items: &[Mutation],
        indexes: &index::IndexManager,
    ) -> Result<usize> {
        // Borrow-checker: run inside one transaction by routing each leaf through
        // a tx-scoped applier that also keeps FTS in sync.
        self.transact(|tx| {
            let mut count = 0usize;
            for m in items {
                count += apply_mutation_tx(tx, m, indexes)?;
            }
            Ok(count)
        })
    }

    // --- Oplog (append-only substrate, DL-4) -----------------------------

    /// Append one op to the oplog. `op_id` is the primary key; appending the
    /// same id twice is a `StorageError` (the substrate is append-only).
    #[allow(clippy::too_many_arguments)]
    pub fn append_op(
        &self,
        op_id: &str,
        actor_id: &str,
        workspace_id: &str,
        lamport: u64,
        kind: &str,
        payload: &[u8],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO oplog
                     (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![op_id, actor_id, workspace_id, lamport as i64, kind, payload, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Read every oplog entry, ordered by `(lamport, op_id)` — a deterministic
    /// total order for replay/rebuild.
    pub fn list_ops(&self) -> Result<Vec<OpRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT op_id, actor_id, workspace_id, lamport, kind, payload
                   FROM oplog ORDER BY lamport, op_id",
            )
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(OpRow {
                    op_id: row.get(0)?,
                    actor_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    lamport: row.get::<_, i64>(3)? as u64,
                    kind: row.get(4)?,
                    payload: row.get(5)?,
                })
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    // --- CRDT blobs (DL-4 chunks, DL-19 snapshots) -----------------------

    /// Append a CRDT op chunk for `doc_id`.
    ///
    /// `crdt_chunks` is the append-only rebuild/sync source of truth (DL-6), so
    /// a `(doc_id, chunk_id)` is **immutable** once written: re-writing the same
    /// chunk with identical `(format, payload)` is an idempotent no-op
    /// (sync-safe — the same op chunk may arrive twice), but re-writing an
    /// existing chunk id with *different* content is rejected with
    /// `StorageError` rather than silently rewriting history (review 003).
    pub fn put_chunk(
        &self,
        doc_id: &str,
        chunk_id: &str,
        format: &str,
        payload: &[u8],
    ) -> Result<()> {
        if let Some(existing) = self.get_chunk(doc_id, chunk_id)? {
            if existing.format == format && existing.payload == payload {
                return Ok(()); // idempotent: identical chunk re-write
            }
            return Err(CoreError::StorageError(format!(
                "crdt chunk ({doc_id}, {chunk_id}) is append-only and already exists \
                 with different content; refusing to rewrite history"
            )));
        }
        self.conn
            .execute(
                "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![doc_id, chunk_id, format, payload, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Read a single chunk by `(doc_id, chunk_id)`, if present.
    pub fn get_chunk(&self, doc_id: &str, chunk_id: &str) -> Result<Option<ChunkRow>> {
        self.conn
            .query_row(
                "SELECT chunk_id, format, payload FROM crdt_chunks
                  WHERE doc_id = ?1 AND chunk_id = ?2",
                params![doc_id, chunk_id],
                |row| {
                    Ok(ChunkRow {
                        chunk_id: row.get(0)?,
                        format: row.get(1)?,
                        payload: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(map_sql)
    }

    /// Read all chunks for `doc_id`, ordered by `(created_at, chunk_id)`.
    pub fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT chunk_id, format, payload FROM crdt_chunks
                  WHERE doc_id = ?1 ORDER BY created_at, chunk_id",
            )
            .map_err(map_sql)?;
        let rows = stmt
            .query_map(params![doc_id], |row| {
                Ok(ChunkRow {
                    chunk_id: row.get(0)?,
                    format: row.get(1)?,
                    payload: row.get(2)?,
                })
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    /// List every distinct `doc_id` that has at least one persisted chunk, sorted.
    ///
    /// This is the union/iteration primitive the in-process sync seam needs
    /// (prd-merged/03 SS-1/SS-2): a peer advertises a frontier *per `doc_id`*, so
    /// the sync runner walks the union of doc ids across two stores. It is the
    /// public form of the `SELECT DISTINCT doc_id` the DL-6
    /// [`rebuild_projection`](Self::rebuild_projection) already does internally —
    /// exposed so the sync crate (and any future transport) can enumerate docs
    /// without reaching the raw connection. Includes non-collection docs (e.g. a
    /// future `src/<file>` text doc); callers filter by prefix as needed.
    pub fn list_doc_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT doc_id FROM crdt_chunks ORDER BY doc_id")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    /// Store a CRDT snapshot for `doc_id`.
    pub fn put_snapshot(
        &self,
        doc_id: &str,
        snapshot_id: &str,
        format: &str,
        payload: &[u8],
        frontier: &[u8],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO crdt_snapshots
                     (doc_id, snapshot_id, format, payload, frontier, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(doc_id, snapshot_id) DO UPDATE SET
                     format = excluded.format,
                     payload = excluded.payload,
                     frontier = excluded.frontier,
                     created_at = excluded.created_at",
                params![doc_id, snapshot_id, format, payload, frontier, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// The most-recent snapshot for `doc_id` (by `created_at`, then
    /// `snapshot_id`), if any.
    pub fn latest_snapshot(&self, doc_id: &str) -> Result<Option<SnapshotRow>> {
        self.conn
            .query_row(
                "SELECT snapshot_id, format, payload, frontier FROM crdt_snapshots
                  WHERE doc_id = ?1 ORDER BY created_at DESC, snapshot_id DESC LIMIT 1",
                params![doc_id],
                |row| {
                    Ok(SnapshotRow {
                        snapshot_id: row.get(0)?,
                        format: row.get(1)?,
                        payload: row.get(2)?,
                        frontier: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(map_sql)
    }

    // --- Runs (replay source, prd-merged/01 CR-9) ------------------------

    /// Persist a full `RunRecord` as JSON for `runtime.replay`. Re-saving the
    /// same `run_id` overwrites (idempotent record-and-replace).
    ///
    /// The record's `code_hash` is its provenance + replay key, so it is
    /// validated against the canonical `sha256:` form before it is allowed to
    /// land in the substrate (prd-merged/01 CR-9; review 013/014). A record
    /// carrying a divergent string (the runtime's old `fnv1a64:…`, an uppercase
    /// digest, a truncated body) is rejected with a `ValidationError` here,
    /// rather than persisting a row the pipeline could never reproduce.
    pub fn save_run(&self, run: &RunRecord) -> Result<()> {
        run.validate_code_hash()?;
        let json = serde_json::to_string(run).map_err(|e| map_json("save_run", e))?;
        self.conn
            .execute(
                "INSERT INTO runs (run_id, applet_id, record_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(run_id) DO UPDATE SET
                     applet_id = excluded.applet_id,
                     record_json = excluded.record_json,
                     created_at = excluded.created_at",
                params![run.run_id.as_str(), run.applet_id.as_str(), json, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Load a `RunRecord` by id, reconstructed from its stored JSON.
    ///
    /// The provenance contract is re-checked on read: a corrupted or legacy row
    /// (e.g. a `fnv1a64:…` `code_hash` written before this guard existed, or a
    /// digest mangled in the file) surfaces a `ValidationError` instead of
    /// silently handing back a record the pipeline can never reproduce
    /// (prd-merged/01 CR-9; review 013/014).
    pub fn load_run(&self, run_id: &str) -> Result<Option<RunRecord>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT record_json FROM runs WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        match json {
            Some(s) => {
                let run: RunRecord =
                    serde_json::from_str(&s).map_err(|e| map_json("load_run", e))?;
                run.validate_code_hash()?;
                Ok(Some(run))
            }
            None => Ok(None),
        }
    }
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

/// Bind a JSON scalar as a SQLite value for a parameterized predicate. Numbers
/// bind as INTEGER/REAL (so JSON1 numeric comparisons line up), booleans as the
/// JSON1 `0`/`1` integers `json_extract` returns, strings as TEXT, and null as
/// SQL NULL. Arrays/objects are never bound (the planner rejects them upstream).
fn json_to_sql(value: &serde_json::Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as V;
    let out = match value {
        serde_json::Value::Null => V::Null,
        serde_json::Value::Bool(b) => V::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                V::Integer(i)
            } else if let Some(f) = n.as_f64() {
                V::Real(f)
            } else {
                // u64 outside i64 range: store as text to avoid lossy coercion.
                V::Text(n.to_string())
            }
        }
        serde_json::Value::String(s) => V::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            return Err(CoreError::QueryError(
                "cannot bind a non-scalar value as a SQL parameter".into(),
            ))
        }
    };
    Ok(out)
}

/// Convert the planner's ordered JSON bind list into rusqlite values.
fn to_sql_params(values: &[serde_json::Value]) -> Result<Vec<rusqlite::types::Value>> {
    values.iter().map(json_to_sql).collect()
}

/// Advance a record's `updated_at` to `logical_at` when supplied (never
/// backwards), leaving it untouched otherwise.
fn bump_updated_at(env: &mut RecordEnvelope, logical_at: Option<i64>) {
    if let Some(at) = logical_at {
        let ts = forge_domain::LogicalTimestamp(at.max(0) as u64);
        if ts > env.updated_at {
            env.updated_at = ts;
        }
    }
}

/// The stable `field_id` (DL-7) a display field name maps to in the M0a mutation
/// surface: `f_<name>`. This is the projection-side stand-in for the schema
/// registry's name→stable-id mapping (the registry mints ids from `ActorId` for
/// schema-defined fields; the DL-17 mutation surface writes display fields and
/// must materialize the matching stable id so expression/FTS indexes keyed by
/// `$.field_ids.<id>` actually see the record — review 045/046 finding 1).
fn field_id_for_name(name: &str) -> String {
    format!("f_{name}")
}

/// Re-derive `env.field_ids` from the record's display `fields` so a record
/// written through the DL-17 mutation surface is visible to indexes keyed by the
/// stable field id (review 045/046 finding 1). Without this, `RecordEnvelope::new`
/// leaves `field_ids` empty and an inserted/patched record is invisible to active
/// expression/FTS indexes until a manual rebuild.
///
/// Each display field `<name>` materializes/refreshes `field_ids["f_<name>"] =
/// value`, **layered on top of** any existing stable ids rather than rebuilding
/// the map. Existing schema-minted ids (e.g. `f_alice_0`, `f_dev.01_0`) are
/// PRESERVED — a display-name `update`/`patch` must never drop them, which would
/// strand the record from an expression/FTS index keyed to the real schema id and
/// (because active FTS sync deletes-then-reinserts) make it disappear from search
/// after an unrelated mutation (review 049). A brand-new insert starts with empty
/// `field_ids`, so this yields exactly the `f_<name>` stand-ins.
///
/// Trade-off (M0a): storage has no schema name→id map, so a field patched away
/// leaves its stand-in behind rather than risk dropping a real schema id; a stale
/// stand-in is a far lesser fault than corrupting a record's stable identity.
fn materialize_field_ids(env: &mut RecordEnvelope) {
    for (name, value) in &env.fields {
        env.field_ids.insert(field_id_for_name(name), value.clone());
    }
}

// --- Transaction-scoped record helpers (for grouped mutations) -------------

/// Read a record inside an open transaction.
fn get_record_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
    id: &str,
) -> Result<Option<RecordEnvelope>> {
    let data: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE collection = ?1 AND id = ?2",
            params![collection, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?;
    match data {
        Some(json) => Ok(Some(
            serde_json::from_str(&json).map_err(|e| map_json("get_record_tx", e))?,
        )),
        None => Ok(None),
    }
}

/// Upsert a record inside an open transaction.
fn put_record_tx(tx: &rusqlite::Transaction<'_>, env: &RecordEnvelope) -> Result<()> {
    let data = serde_json::to_string(env).map_err(|e| map_json("put_record_tx", e))?;
    tx.execute(
        "INSERT INTO records (collection, id, data, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(collection, id) DO UPDATE SET
             data = excluded.data,
             updated_at = excluded.updated_at",
        params![
            env.collection.as_str(),
            env.entity_id.as_str(),
            data,
            now_ms()
        ],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Upsert a record inside an open transaction AND refresh active FTS rows for it
/// in the same transaction (DL-5). The single seam every tx-scoped mutation uses,
/// so no applet write can leave an active FTS shadow table stale (review
/// 041/042 finding 3).
fn put_record_synced_tx(
    tx: &rusqlite::Transaction<'_>,
    env: &RecordEnvelope,
    indexes: &index::IndexManager,
) -> Result<()> {
    let data = serde_json::to_string(env).map_err(|e| map_json("put_record_synced_tx", e))?;
    put_record_tx(tx, env)?;
    indexes.sync_fts_for_record(
        tx,
        env.collection.as_str(),
        env.entity_id.as_str(),
        &data,
    )
}

/// Apply one mutation inside an open transaction, returning the number of leaf
/// mutations applied (so a nested `transact` counts each contained leaf). Every
/// projection write goes through the transaction and refreshes any active FTS
/// shadow rows in the same transaction, so a later failure rolls the whole group
/// back (DL-17 atomic-local) and an active FTS index never goes stale (DL-5).
fn apply_mutation_tx(
    tx: &rusqlite::Transaction<'_>,
    m: &Mutation,
    indexes: &index::IndexManager,
) -> Result<usize> {
    match m {
        Mutation::Insert {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let id = id.as_ref().ok_or_else(|| {
                CoreError::QueryError("insert requires a collection-scoped id".into())
            })?;
            let at = forge_domain::LogicalTimestamp(logical_at.unwrap_or(0).max(0) as u64);
            let mut env = RecordEnvelope::new(
                forge_domain::CollectionId::new(collection.clone()),
                forge_domain::RecordId::new(id.clone()),
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                at,
            );
            // Materialize the stable field ids the projection indexes read, so an
            // inserted record is visible to active expression/FTS indexes (review
            // 045/046 finding 1).
            materialize_field_ids(&mut env);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Update {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("update: record {collection}/{id} does not exist"))
            })?;
            env.fields = fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            materialize_field_ids(&mut env);
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Patch {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
            })?;
            for (k, v) in fields {
                env.fields.insert(k.clone(), v.clone());
            }
            materialize_field_ids(&mut env);
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Delete {
            collection,
            id,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
            })?;
            env.deleted = true;
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Transact { items } => {
            let mut count = 0usize;
            for item in items {
                count += apply_mutation_tx(tx, item, indexes)?;
            }
            Ok(count)
        }
    }
}

/// A row read back from the oplog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRow {
    pub op_id: String,
    pub actor_id: String,
    pub workspace_id: String,
    pub lamport: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}

/// A CRDT op chunk read back for a doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRow {
    pub chunk_id: String,
    pub format: String,
    pub payload: Vec<u8>,
}

/// A CRDT snapshot read back for a doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    pub snapshot_id: String,
    pub format: String,
    pub payload: Vec<u8>,
    pub frontier: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{
        AppResult, AppletId, CollectionId, LogicalTimestamp, RecordId, RecordedCall, RunId,
        RunOutcome, RunRecord,
    };
    use std::collections::BTreeMap;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    fn fields(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn sample_record(collection: &str, id: &str, title: &str) -> RecordEnvelope {
        RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new(id),
            fields(&[("title", serde_json::json!(title))]),
            LogicalTimestamp(1),
        )
    }

    // Model lifted from domain::run tests so the roundtrip exercises a fully
    // populated record (calls, logs, completed outcome).
    fn sample_run(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            // Canonical sha256: of a stand-in body, so the sample is a
            // contract-valid record (its code_hash passes validate_code_hash,
            // which save_run/load_run now enforce — review 013/014).
            code_hash: forge_domain::code_hash("sample-body"),
            input: serde_json::json!({"name": "world"}),
            random_seed: 42,
            time_start: 1000,
            calls: vec![
                RecordedCall {
                    seq: 0,
                    method: "time.now".into(),
                    args: serde_json::json!(null),
                    response: serde_json::json!(1000),
                },
                RecordedCall {
                    seq: 1,
                    method: "storage.set".into(),
                    args: serde_json::json!(["name", "world"]),
                    response: serde_json::json!(null),
                },
            ],
            logs: vec!["hello".into()],
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: serde_json::json!("Hello world") },
            },
        }
    }

    // --- open / schema ---------------------------------------------------

    #[test]
    fn open_in_memory_sets_wal_and_pragmas() {
        let s = store();
        let fk: i64 = s
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON");
        // synchronous=NORMAL is integer 1.
        let sync: i64 = s
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 1, "synchronous must be NORMAL(=1)");
    }

    #[test]
    fn open_is_idempotent_on_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        {
            let s = Store::open(&path).unwrap();
            s.kv_set("ns", "k", b"v", "text/plain").unwrap();
        }
        // Re-opening the same file must not error on CREATE TABLE IF NOT EXISTS.
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.kv_get("ns", "k").unwrap().as_deref(), Some(&b"v"[..]));
    }

    // --- KV --------------------------------------------------------------

    #[test]
    fn kv_roundtrip_and_overwrite() {
        let s = store();
        assert_eq!(s.kv_get("app", "k1").unwrap(), None);
        s.kv_set("app", "k1", b"hello", "text/plain").unwrap();
        assert_eq!(s.kv_get("app", "k1").unwrap().as_deref(), Some(&b"hello"[..]));
        s.kv_set("app", "k1", b"world", "text/plain").unwrap();
        assert_eq!(s.kv_get("app", "k1").unwrap().as_deref(), Some(&b"world"[..]));
        // logical_version bumps on overwrite.
        let ver: i64 = s
            .conn
            .query_row(
                "SELECT logical_version FROM kv WHERE namespace='app' AND key='k1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ver, 2);
    }

    #[test]
    fn kv_namespaces_are_isolated() {
        let s = store();
        s.kv_set("a", "k", b"1", "text/plain").unwrap();
        s.kv_set("b", "k", b"2", "text/plain").unwrap();
        assert_eq!(s.kv_get("a", "k").unwrap().as_deref(), Some(&b"1"[..]));
        assert_eq!(s.kv_get("b", "k").unwrap().as_deref(), Some(&b"2"[..]));
    }

    #[test]
    fn kv_list_prefix_sorted_and_filtered() {
        let s = store();
        s.kv_set("ns", "app/b", b"1", "text/plain").unwrap();
        s.kv_set("ns", "app/a", b"1", "text/plain").unwrap();
        s.kv_set("ns", "other/x", b"1", "text/plain").unwrap();
        s.kv_set("other_ns", "app/z", b"1", "text/plain").unwrap();
        let keys = s.kv_list("ns", "app/").unwrap();
        assert_eq!(keys, vec!["app/a".to_string(), "app/b".to_string()]);
        // Empty prefix lists everything in the namespace.
        let all = s.kv_list("ns", "").unwrap();
        assert_eq!(all, vec!["app/a", "app/b", "other/x"]);
    }

    #[test]
    fn kv_list_escapes_like_metacharacters() {
        let s = store();
        s.kv_set("ns", "a%b", b"1", "text/plain").unwrap();
        s.kv_set("ns", "axb", b"1", "text/plain").unwrap();
        // Prefix "a%" must match only the literal "a%b", not "axb".
        let keys = s.kv_list("ns", "a%").unwrap();
        assert_eq!(keys, vec!["a%b".to_string()]);
    }

    #[test]
    fn kv_delete_tombstones_and_hides_from_get_and_list() {
        let s = store();
        s.kv_set("ns", "k", b"v", "text/plain").unwrap();
        s.kv_delete("ns", "k").unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap(), None);
        assert!(s.kv_list("ns", "").unwrap().is_empty());
        // Row still exists, tombstone=1.
        let tomb: i64 = s
            .conn
            .query_row(
                "SELECT tombstone FROM kv WHERE namespace='ns' AND key='k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tomb, 1);
        // Re-setting clears the tombstone.
        s.kv_set("ns", "k", b"v2", "text/plain").unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap().as_deref(), Some(&b"v2"[..]));
        assert_eq!(s.kv_list("ns", "").unwrap(), vec!["k".to_string()]);
    }

    #[test]
    fn kv_delete_of_missing_key_is_noop() {
        let s = store();
        // Should not error even though nothing matches.
        s.kv_delete("ns", "nope").unwrap();
        assert_eq!(s.kv_get("ns", "nope").unwrap(), None);
    }

    /// Review 036 finding 3: `next_counter` reserves a strictly increasing,
    /// never-repeating value, starting at 1, and the reserved value is durably
    /// persisted (so a re-open continues monotonically). This is the atomic
    /// primitive the core uses to mint a unique per-execution `run_id`.
    #[test]
    fn next_counter_is_monotone_unique_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let mut seen = std::collections::BTreeSet::new();
        {
            let mut s = Store::open(&path).unwrap();
            for expected in 1..=5u64 {
                let n = s.next_counter("__forge/meta", "run_counter").unwrap();
                assert_eq!(n, expected, "counter must increment by one from 1");
                assert!(seen.insert(n), "every reservation must be unique: {n}");
            }
        }
        // Re-open the SAME file: the counter is durable, so the next reservation
        // continues past the last persisted value rather than restarting.
        {
            let mut s2 = Store::open(&path).unwrap();
            let n = s2.next_counter("__forge/meta", "run_counter").unwrap();
            assert_eq!(n, 6, "re-open must continue the persisted counter");
            assert!(seen.insert(n));
        }
    }

    /// A corrupted (non-integer) persisted counter is a `StorageError`, not a
    /// silent reset to zero (which would re-issue already-used reservations).
    #[test]
    fn next_counter_rejects_a_corrupted_value() {
        let mut s = store();
        s.kv_set("__forge/meta", "run_counter", b"not-a-number", "text/plain").unwrap();
        let err = s.next_counter("__forge/meta", "run_counter").unwrap_err();
        assert_eq!(err.code(), "StorageError", "{err}");
    }

    /// Review 038 finding 3: two *separate* file-backed `Store` handles (the
    /// two-`WorkspaceCore` race) bumping the SAME counter concurrently must each
    /// receive a DISTINCT value — never a collision and never a spurious
    /// `database is locked`. With the old `DEFERRED` SELECT-then-upsert both
    /// connections could read the same snapshot; `BEGIN IMMEDIATE` + busy-timeout
    /// + retry serializes them so every reservation is unique and contiguous.
    #[test]
    fn next_counter_two_file_handles_never_collide_or_lock() {
        use std::collections::BTreeSet;
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        // Create the file/schema once up front so both threads open an existing DB.
        Store::open(&path).unwrap();

        const PER_THREAD: u64 = 50;
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                // A genuinely independent handle (its own SQLite connection),
                // exactly the two-`WorkspaceCore::open()` scenario.
                let mut s = Store::open(&path).unwrap();
                barrier.wait(); // maximize the contention window
                let mut got = Vec::with_capacity(PER_THREAD as usize);
                for _ in 0..PER_THREAD {
                    got.push(
                        s.next_counter("__forge/meta", "run_counter")
                            .expect("a contended reservation must not surface a lock error"),
                    );
                }
                got
            }));
        }

        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().expect("counter thread panicked"));
        }

        // Every reservation across BOTH handles is unique (no collision → no
        // silently-overwritten run record).
        let unique: BTreeSet<u64> = all.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "two file handles must never reserve the same counter value"
        );
        // The set is exactly 1..=(2*PER_THREAD): no gaps and no duplicates, so the
        // two interleavings were serialized, not lost.
        let total = 2 * PER_THREAD;
        let expected: BTreeSet<u64> = (1..=total).collect();
        assert_eq!(unique, expected, "reservations must be contiguous 1..=2N");
    }

    // --- Records projection ----------------------------------------------

    #[test]
    fn record_put_get_roundtrips_full_envelope() {
        let s = store();
        let mut rec = sample_record("tasks", "rec_1", "Ship");
        rec.unknown_fields.insert("f_future".into(), serde_json::json!({"x": 1}));
        s.put_record(&rec).unwrap();
        let back = s.get_record("tasks", "rec_1").unwrap().unwrap();
        assert_eq!(back, rec, "stored envelope must reconstruct identically");
        assert_eq!(back.unknown_fields["f_future"], serde_json::json!({"x": 1}));
    }

    #[test]
    fn record_get_missing_is_none() {
        let s = store();
        assert_eq!(s.get_record("tasks", "nope").unwrap(), None);
    }

    #[test]
    fn record_put_overwrites_projection() {
        let s = store();
        s.put_record(&sample_record("tasks", "rec_1", "old")).unwrap();
        s.put_record(&sample_record("tasks", "rec_1", "new")).unwrap();
        let back = s.get_record("tasks", "rec_1").unwrap().unwrap();
        assert_eq!(back.fields["title"], serde_json::json!("new"));
        // Still exactly one row for that PK.
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='tasks' AND id='rec_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn record_data_is_valid_json_for_json1() {
        let s = store();
        s.put_record(&sample_record("tasks", "rec_1", "Ship MVP")).unwrap();
        // Exercise the JSON1 path the projection promises (DL-4/DL-5).
        let title: String = s
            .conn
            .query_row(
                "SELECT json_extract(data, '$.fields.title') FROM records
                  WHERE collection='tasks' AND id='rec_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Ship MVP");
    }

    #[test]
    fn list_records_orders_by_id_and_scopes_collection() {
        let s = store();
        s.put_record(&sample_record("tasks", "b", "B")).unwrap();
        s.put_record(&sample_record("tasks", "a", "A")).unwrap();
        s.put_record(&sample_record("notes", "z", "Z")).unwrap();
        let recs = s.list_records("tasks").unwrap();
        let ids: Vec<_> = recs.iter().map(|r| r.entity_id.as_str().to_string()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.list_records("notes").unwrap().len(), 1);
        assert!(s.list_records("empty").unwrap().is_empty());
    }

    // --- Oplog -----------------------------------------------------------

    #[test]
    fn oplog_append_and_read_in_order() {
        let s = store();
        s.append_op("op2", "actor", "ws", 2, "insert", b"p2").unwrap();
        s.append_op("op1", "actor", "ws", 1, "insert", b"p1").unwrap();
        let ops = s.list_ops().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].op_id, "op1");
        assert_eq!(ops[0].lamport, 1);
        assert_eq!(ops[0].payload, b"p1");
        assert_eq!(ops[1].op_id, "op2");
        assert_eq!(ops[1].lamport, 2);
    }

    #[test]
    fn oplog_duplicate_op_id_is_storage_error() {
        let s = store();
        s.append_op("op1", "a", "ws", 1, "k", b"x").unwrap();
        let err = s.append_op("op1", "a", "ws", 1, "k", b"y").unwrap_err();
        assert_eq!(err.code(), "StorageError");
    }

    // --- CRDT blobs ------------------------------------------------------

    #[test]
    fn chunks_store_and_read() {
        let s = store();
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();
        s.put_chunk("doc1", "c2", "loro", b"bbb").unwrap();
        s.put_chunk("doc2", "c1", "loro", b"zzz").unwrap();
        let chunks = s.get_chunks("doc1").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_id, "c1");
        assert_eq!(chunks[0].payload, b"aaa");
        assert_eq!(chunks[1].payload, b"bbb");
        assert_eq!(s.get_chunks("doc2").unwrap().len(), 1);
        assert!(s.get_chunks("missing").unwrap().is_empty());
    }

    #[test]
    fn list_doc_ids_returns_distinct_sorted_docs() {
        // The sync seam's per-doc frontier walk (SS-1/SS-2) needs the union of
        // doc ids that hold chunks: distinct, sorted, and empty when there are
        // none.
        let s = store();
        assert!(s.list_doc_ids().unwrap().is_empty());
        s.put_chunk("collection/notes", "c1", "loro", b"n").unwrap();
        s.put_chunk("collection/tasks", "c1", "loro", b"a").unwrap();
        s.put_chunk("collection/tasks", "c2", "loro", b"b").unwrap();
        assert_eq!(
            s.list_doc_ids().unwrap(),
            vec!["collection/notes".to_string(), "collection/tasks".to_string()],
            "doc ids are distinct and sorted regardless of chunk count"
        );
    }

    #[test]
    fn put_chunk_is_append_only_immutable() {
        // Append-only history (review 003): an identical re-write is an
        // idempotent no-op, but a conflicting re-write of the same chunk id is
        // refused with StorageError instead of silently rewriting history.
        let s = store();
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();

        // Idempotent: same content re-written is fine and does not duplicate.
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();
        assert_eq!(s.get_chunks("doc1").unwrap().len(), 1);

        // Conflicting payload for an existing chunk id -> StorageError.
        let err = s.put_chunk("doc1", "c1", "loro", b"DIFFERENT").unwrap_err();
        assert_eq!(err.code(), "StorageError");

        // History is unchanged: the original chunk still has its payload.
        let got = s.get_chunk("doc1", "c1").unwrap().unwrap();
        assert_eq!(got.payload, b"aaa");
    }

    #[test]
    fn snapshots_store_and_latest_wins() {
        let s = store();
        assert_eq!(s.latest_snapshot("doc1").unwrap(), None);
        s.put_snapshot("doc1", "s1", "loro", b"snap1", b"f1").unwrap();
        s.put_snapshot("doc1", "s2", "loro", b"snap2", b"f2").unwrap();
        let latest = s.latest_snapshot("doc1").unwrap().unwrap();
        // s2 inserted after s1 -> later created_at / higher id wins.
        assert_eq!(latest.snapshot_id, "s2");
        assert_eq!(latest.payload, b"snap2");
        assert_eq!(latest.frontier, b"f2");
    }

    #[test]
    fn snapshot_upsert_replaces_payload() {
        let s = store();
        s.put_snapshot("doc1", "s1", "loro", b"v1", b"f1").unwrap();
        s.put_snapshot("doc1", "s1", "loro", b"v2", b"f2").unwrap();
        let latest = s.latest_snapshot("doc1").unwrap().unwrap();
        assert_eq!(latest.payload, b"v2");
        assert_eq!(latest.frontier, b"f2");
    }

    // --- Runs ------------------------------------------------------------

    #[test]
    fn save_run_load_run_roundtrips() {
        let s = store();
        let run = sample_run("run_1");
        s.save_run(&run).unwrap();
        let back = s.load_run("run_1").unwrap().unwrap();
        assert_eq!(back, run, "loaded run must equal the original (replay source)");
        // And the replay fingerprint matches, proving the trace survived.
        assert!(run.replays_identically(&back));
    }

    #[test]
    fn load_run_missing_is_none() {
        let s = store();
        assert_eq!(s.load_run("nope").unwrap(), None);
    }

    #[test]
    fn save_run_rejects_noncanonical_code_hash() {
        // Provenance contract (review 013/014): a record carrying a divergent
        // code_hash (the runtime's old fnv1a64: form) must be refused on save
        // rather than persisting a row the pipeline can never reproduce.
        let s = store();
        let mut run = sample_run("run_bad");
        run.code_hash = "fnv1a64:0123456789abcdef".into();
        let err = s.save_run(&run).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        // Nothing was persisted: the rejected record never reached the table.
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "rejected record must not land in the runs table");
    }

    #[test]
    fn load_run_rejects_corrupted_legacy_code_hash() {
        // A legacy/corrupted row (written before this guard, or mangled in the
        // file) must surface a ValidationError on read, not a silent bad record
        // (review 013/014). Inject the bad row directly, bypassing save_run's
        // guard, to model the on-disk legacy/corruption case.
        let s = store();
        let mut run = sample_run("run_legacy");
        run.code_hash = "fnv1a64:deadbeef".into();
        let json = serde_json::to_string(&run).unwrap();
        s.conn
            .execute(
                "INSERT INTO runs (run_id, applet_id, record_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params!["run_legacy", "app_notes", json, 0i64],
            )
            .unwrap();
        let err = s.load_run("run_legacy").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn save_run_load_run_validates_canonical_roundtrip() {
        // A round-tripped canonical record loads and re-validates cleanly.
        let s = store();
        let run = sample_run("run_ok");
        assert!(run.validate_code_hash().is_ok(), "sample must be canonical");
        s.save_run(&run).unwrap();
        let back = s.load_run("run_ok").unwrap().unwrap();
        assert_eq!(back, run);
        assert!(back.validate_code_hash().is_ok());
    }

    #[test]
    fn save_run_overwrites_same_id() {
        let s = store();
        let mut run = sample_run("run_1");
        s.save_run(&run).unwrap();
        run.random_seed = 99;
        s.save_run(&run).unwrap();
        let back = s.load_run("run_1").unwrap().unwrap();
        assert_eq!(back.random_seed, 99);
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    // --- transact (DL-4) -------------------------------------------------

    #[test]
    fn transact_commits_on_ok() {
        let mut s = store();
        s.transact(|tx| {
            tx.execute(
                "INSERT INTO kv (namespace, key, value, content_type, logical_version, updated_at, tombstone)
                 VALUES ('ns','k', x'01', 'text/plain', 1, 0, 0)",
                [],
            )
            .map_err(map_sql)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap().as_deref(), Some(&[1u8][..]));
    }

    #[test]
    fn transact_rolls_back_on_err_leaving_db_unchanged() {
        let mut s = store();
        s.kv_set("ns", "pre", b"x", "text/plain").unwrap();
        let before: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0))
            .unwrap();

        let result: Result<()> = s.transact(|tx| {
            tx.execute(
                "INSERT INTO kv (namespace, key, value, content_type, logical_version, updated_at, tombstone)
                 VALUES ('ns','mid', x'02', 'text/plain', 1, 0, 0)",
                [],
            )
            .map_err(map_sql)?;
            // Now bail; the mid insert must be rolled back.
            Err(CoreError::ValidationError("boom".into()))
        });
        assert_eq!(result.unwrap_err().code(), "ValidationError");

        let after: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after, "rolled-back insert must not persist");
        assert_eq!(s.kv_get("ns", "mid").unwrap(), None);
        // Pre-existing data untouched.
        assert_eq!(s.kv_get("ns", "pre").unwrap().as_deref(), Some(&b"x"[..]));
    }

    #[test]
    fn transact_returns_closure_value() {
        let mut s = store();
        let n = s.transact(|_tx| Ok(7usize)).unwrap();
        assert_eq!(n, 7);
    }

    // --- durability / persistence ----------------------------------------

    #[test]
    fn file_backed_db_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        {
            let s = Store::open(&path).unwrap();
            s.kv_set("ns", "k", b"durable", "text/plain").unwrap();
            s.put_record(&sample_record("tasks", "rec_1", "kept")).unwrap();
            s.append_op("op1", "a", "ws", 1, "insert", b"p").unwrap();
            s.put_snapshot("doc1", "s1", "loro", b"snap", b"fr").unwrap();
            s.save_run(&sample_run("run_1")).unwrap();
        } // Store (and Connection) dropped — WAL flushed on close.

        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.kv_get("ns", "k").unwrap().as_deref(), Some(&b"durable"[..]));
        assert_eq!(
            s2.get_record("tasks", "rec_1").unwrap().unwrap().fields["title"],
            serde_json::json!("kept")
        );
        assert_eq!(s2.list_ops().unwrap().len(), 1);
        assert_eq!(s2.latest_snapshot("doc1").unwrap().unwrap().payload, b"snap");
        assert_eq!(s2.load_run("run_1").unwrap().unwrap().run_id, RunId::new("run_1"));
    }

    #[test]
    fn file_backed_db_uses_wal_journal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let s = Store::open(&path).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "DL-23 requires WAL on disk");
    }

    // --- Query engine (DL-15) --------------------------------------------

    fn seed_tasks(s: &Store) {
        // Three tasks with display fields; prio numeric, status/title text.
        let mut a = sample_record("tasks", "tasks/1", "alpha");
        a.fields.insert("status".into(), serde_json::json!("todo"));
        a.fields.insert("prio".into(), serde_json::json!(1));
        let mut b = sample_record("tasks", "tasks/2", "beta");
        b.fields.insert("status".into(), serde_json::json!("done"));
        b.fields.insert("prio".into(), serde_json::json!(2));
        let mut c = sample_record("tasks", "tasks/3", "gamma");
        c.fields.insert("status".into(), serde_json::json!("todo"));
        c.fields.insert("prio".into(), serde_json::json!(3));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        s.put_record(&c).unwrap();
    }

    fn plan(v: serde_json::Value) -> Query {
        Query::from_fixture_value(&v).expect("parse plan")
    }

    #[test]
    fn query_eq_filters_and_orders() {
        let s = store();
        seed_tasks(&s);
        let q = plan(serde_json::json!({
            "from": "tasks", "where": ["status", "=", "todo"], "orderBy": ["prio", "asc"]
        }));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/3"]);
    }

    #[test]
    fn query_ne_excludes_match_and_treats_missing_as_differing() {
        let s = store();
        seed_tasks(&s);
        // A record without `status` at all: ne('done') should still include it
        // (missing differs from 'done').
        let mut d = sample_record("tasks", "tasks/4", "delta");
        d.fields.remove("status"); // sample_record only sets title anyway
        s.put_record(&d).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["status", "!=", "done"]}));
        let ids = s.query(&q).unwrap().ids();
        assert!(ids.contains(&"tasks/1".to_string()));
        assert!(ids.contains(&"tasks/3".to_string()));
        assert!(ids.contains(&"tasks/4".to_string()), "missing field differs from a value");
        assert!(!ids.contains(&"tasks/2".to_string()), "status=done excluded");
    }

    #[test]
    fn query_range_excludes_missing_and_non_numeric() {
        let s = store();
        seed_tasks(&s);
        // tasks/5 has a non-numeric prio; a range filter must not coerce it in.
        let mut e = sample_record("tasks", "tasks/5", "eps");
        e.fields.insert("prio".into(), serde_json::json!("99"));
        s.put_record(&e).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["prio", ">=", 2]}));
        let ids = s.query(&q).unwrap().ids();
        assert_eq!(ids, vec!["tasks/2", "tasks/3"], "string prio not coerced into range");
    }

    #[test]
    fn query_in_and_like() {
        let s = store();
        seed_tasks(&s);
        let q_in = plan(serde_json::json!({"from": "tasks", "where": ["status", "in", ["todo"]]}));
        assert_eq!(s.query(&q_in).unwrap().ids(), vec!["tasks/1", "tasks/3"]);

        let q_like = plan(serde_json::json!({"from": "tasks", "where": ["title", "like", "%a"]}));
        // alpha, beta, gamma all end in 'a'.
        assert_eq!(s.query(&q_like).unwrap().ids(), vec!["tasks/1", "tasks/2", "tasks/3"]);
    }

    #[test]
    fn query_like_escapes_metacharacters() {
        let s = store();
        let mut a = sample_record("notes", "n1", "a%b");
        a.fields.insert("title".into(), serde_json::json!("a%b"));
        let mut b = sample_record("notes", "n2", "axb");
        b.fields.insert("title".into(), serde_json::json!("axb"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        // Pattern 'a\%b' (escaped) must match only the literal 'a%b'.
        let q = plan(serde_json::json!({"from": "notes", "where": ["title", "like", "a\\%b"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["n1"]);
    }

    #[test]
    fn query_like_is_ascii_case_insensitive() {
        let s = store();
        let mut a = sample_record("notes", "n1", "Plan");
        a.fields.insert("title".into(), serde_json::json!("Plan"));
        s.put_record(&a).unwrap();
        let q = plan(serde_json::json!({"from": "notes", "where": ["title", "like", "plan"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["n1"], "LIKE folds ASCII case");
    }

    #[test]
    fn query_eq_null_matches_missing_field() {
        let s = store();
        let mut a = sample_record("tasks", "tasks/1", "a"); // no `assignee`
        a.fields.remove("assignee");
        let mut b = sample_record("tasks", "tasks/2", "b");
        b.fields.insert("assignee".into(), serde_json::json!("ada"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["assignee", "=", null]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1"], "missing reads as null for eq(null)");
        let q2 = plan(serde_json::json!({"from": "tasks", "where": ["assignee", "!=", null]}));
        assert_eq!(s.query(&q2).unwrap().ids(), vec!["tasks/2"]);
    }

    #[test]
    fn query_hides_tombstoned_unless_include_deleted() {
        let s = store();
        seed_tasks(&s);
        s.delete_record("tasks", "tasks/2", Some(9)).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "orderBy": ["prio", "asc"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/3"]);
        let q_all = plan(serde_json::json!({
            "from": "tasks", "orderBy": ["prio", "asc"], "includeDeleted": true
        }));
        assert_eq!(s.query(&q_all).unwrap().ids(), vec!["tasks/1", "tasks/2", "tasks/3"]);
    }

    #[test]
    fn query_count_and_group_by() {
        let s = store();
        seed_tasks(&s);
        let q = plan(serde_json::json!({
            "from": "tasks", "where": ["status", "=", "todo"], "aggregate": {"op": "count"}
        }));
        match s.query(&q).unwrap() {
            QueryResult::Aggregate(a) => assert_eq!(a.count, Some(2)),
            other => panic!("expected aggregate, got {other:?}"),
        }
        let qg = plan(serde_json::json!({
            "from": "tasks", "groupBy": "status", "aggregate": {"sum": "prio"}
        }));
        match s.query(&qg).unwrap() {
            QueryResult::Groups(g) => {
                assert_eq!(g.len(), 2);
                // Keys ascending: done, todo.
                assert_eq!(g[0].key, serde_json::json!("done"));
                assert_eq!(g[0].aggregate.sum, Some(2.0));
                assert_eq!(g[1].key, serde_json::json!("todo"));
                assert_eq!(g[1].aggregate.sum, Some(4.0)); // 1 + 3
            }
            other => panic!("expected groups, got {other:?}"),
        }
    }

    /// Review 040 finding 3: `eq`/`ne`/`in` over a JSON boolean must not coerce
    /// to numbers. On one collection where the same field holds a boolean on some
    /// records and a numeric 0/1 on others, `done.eq(false)` matches ONLY the
    /// boolean-false record (not the stored numeric 0), and `eq(true)` matches
    /// only boolean-true (not numeric 1). `json_extract` renders both as SQL 0/1,
    /// so without the json_type guard this silently over-matches.
    #[test]
    fn query_boolean_eq_does_not_coerce_numeric_zero_or_one() {
        let s = store();
        let mut bf = sample_record("flags", "bool_false", "bf");
        bf.fields.insert("done".into(), serde_json::json!(false));
        let mut bt = sample_record("flags", "bool_true", "bt");
        bt.fields.insert("done".into(), serde_json::json!(true));
        let mut n0 = sample_record("flags", "num_0", "n0");
        n0.fields.insert("done".into(), serde_json::json!(0));
        let mut n1 = sample_record("flags", "num_1", "n1");
        n1.fields.insert("done".into(), serde_json::json!(1));
        for r in [&bf, &bt, &n0, &n1] {
            s.put_record(r).unwrap();
        }

        // eq(false) -> only the boolean false, NOT numeric 0.
        let q = plan(serde_json::json!({"from": "flags", "where": ["done", "=", false]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["bool_false"], "eq(false) must not match numeric 0");
        // eq(true) -> only the boolean true, NOT numeric 1.
        let qt = plan(serde_json::json!({"from": "flags", "where": ["done", "=", true]}));
        assert_eq!(s.query(&qt).unwrap().ids(), vec!["bool_true"], "eq(true) must not match numeric 1");
    }

    #[test]
    fn query_boolean_ne_and_in_do_not_coerce_numbers() {
        let s = store();
        let mut bf = sample_record("flags", "bool_false", "bf");
        bf.fields.insert("done".into(), serde_json::json!(false));
        let mut bt = sample_record("flags", "bool_true", "bt");
        bt.fields.insert("done".into(), serde_json::json!(true));
        let mut n0 = sample_record("flags", "num_0", "n0");
        n0.fields.insert("done".into(), serde_json::json!(0));
        for r in [&bf, &bt, &n0] {
            s.put_record(r).unwrap();
        }

        // ne(false): boolean-true and numeric-0 both DIFFER from boolean false;
        // only boolean-false is excluded.
        let qne = plan(serde_json::json!({"from": "flags", "where": ["done", "!=", false]}));
        assert_eq!(
            s.query(&qne).unwrap().ids(),
            vec!["bool_true", "num_0"],
            "ne(false) must treat numeric 0 as differing"
        );
        // in [false]: only the boolean false, NOT numeric 0.
        let qin = plan(serde_json::json!({"from": "flags", "where": ["done", "in", [false]]}));
        assert_eq!(s.query(&qin).unwrap().ids(), vec!["bool_false"], "in[false] must not match numeric 0");
    }

    /// Review 041 finding 5: a stable field id containing a `.` (e.g. `f_dev.01_0`,
    /// mintable from an actor id `dev.01`) must address the literal JSON key, not
    /// a nested json1 path. Filter, index DDL, and FTS all read the same quoted
    /// path, so a query over a dotted id returns the right rows and an index over
    /// it is actually consulted.
    #[test]
    fn query_with_dotted_field_id_addresses_literal_key() {
        let s = store();
        let mut a = sample_record("tasks", "t1", "a");
        a.field_ids.insert("f_dev.01_0".into(), serde_json::json!("open"));
        let mut b = sample_record("tasks", "t2", "b");
        b.field_ids.insert("f_dev.01_0".into(), serde_json::json!("closed"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();

        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_dev.01_0", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        // Without the quoted path this returns [] (json1 reads NULL for the dotted
        // path) instead of the matching row.
        assert_eq!(s.query(&q).unwrap().ids(), vec!["t1"], "dotted field id must read the literal key");
    }

    #[test]
    fn value_index_over_dotted_field_id_is_consulted() {
        let s = store();
        let mut a = sample_record("tasks", "t1", "a");
        a.field_ids.insert("f_dev.01_0".into(), serde_json::json!("open"));
        s.put_record(&a).unwrap();
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "tasks", "f_dev.01_0", CreateIndexKind::Value)
            .unwrap();
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_dev.01_0", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let planned = s.query_planned(&q, &mgr).unwrap();
        assert!(planned.uses_index, "index over dotted field id must be usable");
        assert_eq!(planned.ids(), vec!["t1"], "indexed query over dotted id returns the row");
        // The expression index over the quoted path is the one SQLite consults.
        let plan: String = s
            .connection()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM records \
                 WHERE collection = 'tasks' AND json_extract(data, '$.field_ids.\"f_dev.01_0\"') = 'open'",
                [],
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_records_tasks_f_dev.01_0"),
            "SQLite must consult the dotted-id expression index, got: {plan}"
        );
    }

    #[test]
    fn fts_over_dotted_field_id_matches() {
        let s = store();
        let mut env = RecordEnvelope::new(
            CollectionId::new("notes"),
            RecordId::new("n1"),
            fields(&[("body", serde_json::json!("offline rebuild keeps indexes honest"))]),
            LogicalTimestamp(1),
        );
        env.field_ids
            .insert("f_dev.01_0".into(), serde_json::json!("offline rebuild keeps indexes honest"));
        s.put_record(&env).unwrap();
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "notes", "f_dev.01_0", CreateIndexKind::Fts)
            .unwrap();
        // Populated from the literal dotted key, so the term is searchable.
        let hits = mgr.fts_match(s.connection(), "notes", "f_dev.01_0", "offline").unwrap();
        assert_eq!(hits, vec!["n1".to_string()], "FTS over a dotted field id must mirror the literal key");
    }

    // --- Mutations (DL-17) -----------------------------------------------

    #[test]
    fn insert_patch_delete_sequence_post_state() {
        let mut s = store();
        let indexes = IndexManager::new();
        // insert
        let ins = Mutation::Insert {
            collection: "tasks".into(),
            id: Some("task_001".into()),
            fields: serde_json::json!({"title": "Draft", "status": "draft", "prio": 1})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(1),
        };
        s.apply_mutation(&ins, &indexes).unwrap();
        let after_insert = s.get_record("tasks", "task_001").unwrap().unwrap();
        assert_eq!(after_insert.fields["status"], serde_json::json!("draft"));
        assert!(!after_insert.deleted);

        // patch merges (status+prio change, title preserved)
        let patched = s
            .patch_record(
                "tasks",
                "task_001",
                serde_json::json!({"status": "open", "prio": 2})
                    .as_object()
                    .unwrap(),
                Some(2),
            )
            .unwrap();
        assert_eq!(patched.fields["status"], serde_json::json!("open"));
        assert_eq!(patched.fields["prio"], serde_json::json!(2));
        assert_eq!(patched.fields["title"], serde_json::json!("Draft"), "omitted field preserved");
        assert_eq!(patched.updated_at, forge_domain::LogicalTimestamp(2));

        // delete tombstones, retains fields
        let deleted = s.delete_record("tasks", "task_001", Some(3)).unwrap();
        assert!(deleted.deleted);
        assert_eq!(deleted.fields["status"], serde_json::json!("open"));
        assert_eq!(deleted.updated_at, forge_domain::LogicalTimestamp(3));
        assert_eq!(deleted.created_at, forge_domain::LogicalTimestamp(1), "created_at preserved");

        // hidden from a normal query, retained in the table
        let q = Query::from("tasks");
        assert!(s.query(&q).unwrap().ids().is_empty());
        assert!(s.get_record("tasks", "task_001").unwrap().is_some());
    }

    #[test]
    fn update_replaces_known_fields_but_preserves_unknown() {
        let s = store();
        let mut rec = sample_record("tasks", "t1", "old");
        rec.fields.insert("status".into(), serde_json::json!("todo"));
        rec.unknown_fields.insert("f_future".into(), serde_json::json!({"x": 1}));
        s.put_record(&rec).unwrap();
        // update only sets title; status is dropped from display fields, but
        // unknown_fields must survive (DL-9).
        let updated = s
            .update_record(
                "tasks",
                "t1",
                serde_json::json!({"title": "new"}).as_object().unwrap(),
                Some(5),
            )
            .unwrap();
        assert_eq!(updated.fields.get("title"), Some(&serde_json::json!("new")));
        assert_eq!(updated.fields.get("status"), None, "update replaces display fields");
        assert_eq!(updated.unknown_fields["f_future"], serde_json::json!({"x": 1}));
    }

    #[test]
    fn patch_of_missing_record_is_query_error() {
        let s = store();
        let empty = serde_json::Map::new();
        let err = s.patch_record("tasks", "nope", &empty, None).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn transact_group_is_atomic_and_visible() {
        let mut s = store();
        let mut seed = sample_record("tasks", "tasks/1", "Existing");
        seed.fields.insert("done".into(), serde_json::json!(false));
        s.put_record(&seed).unwrap();

        let items = vec![
            Mutation::Insert {
                collection: "tasks".into(),
                id: Some("tasks/2".into()),
                fields: serde_json::json!({"title": "New", "done": false})
                    .as_object()
                    .unwrap()
                    .clone(),
                logical_at: None,
            },
            Mutation::Patch {
                collection: "tasks".into(),
                id: "tasks/1".into(),
                fields: serde_json::json!({"done": true}).as_object().unwrap().clone(),
                logical_at: None,
            },
        ];
        let n = s.transact_mutations(&items, &IndexManager::new()).unwrap();
        assert_eq!(n, 2);
        let q = Query::from("tasks");
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/2"]);
        assert_eq!(
            s.get_record("tasks", "tasks/1").unwrap().unwrap().fields["done"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn transact_group_rolls_back_on_failure() {
        let mut s = store();
        // The group inserts tasks/2 then patches a missing record -> the whole
        // group must roll back, so tasks/2 is NOT visible afterward.
        let items = vec![
            Mutation::Insert {
                collection: "tasks".into(),
                id: Some("tasks/2".into()),
                fields: serde_json::Map::new(),
                logical_at: None,
            },
            Mutation::Patch {
                collection: "tasks".into(),
                id: "missing".into(),
                fields: serde_json::Map::new(),
                logical_at: None,
            },
        ];
        let err = s.transact_mutations(&items, &IndexManager::new()).unwrap_err();
        assert_eq!(err.code(), "QueryError");
        assert!(
            s.get_record("tasks", "tasks/2").unwrap().is_none(),
            "rolled-back insert must not persist"
        );
    }

    #[test]
    fn nested_transact_flattens_into_one_transaction() {
        let mut s = store();
        let items = vec![Mutation::Transact {
            items: vec![
                Mutation::Insert {
                    collection: "tasks".into(),
                    id: Some("a".into()),
                    fields: serde_json::Map::new(),
                    logical_at: None,
                },
                Mutation::Insert {
                    collection: "tasks".into(),
                    id: Some("b".into()),
                    fields: serde_json::Map::new(),
                    logical_at: None,
                },
            ],
        }];
        assert_eq!(s.transact_mutations(&items, &IndexManager::new()).unwrap(), 2);
        assert_eq!(s.list_records("tasks").unwrap().len(), 2);
    }

    // --- Index-synced record writes (DL-5 FTS maintenance) ---------------

    /// A note envelope whose stable `field_ids.f_alice_0` carries `body` (so an
    /// FTS index over that field id has text to mirror).
    fn note(id: &str, body: &str) -> RecordEnvelope {
        let mut env = RecordEnvelope::new(
            CollectionId::new("notes"),
            RecordId::new(id),
            fields(&[("body", serde_json::json!(body))]),
            LogicalTimestamp(1),
        );
        env.field_ids
            .insert("f_alice_0".into(), serde_json::json!(body));
        env
    }

    /// Create an Active FTS index on notes/body via the Store create API.
    fn notes_fts(store: &Store) -> IndexManager {
        let mut mgr = IndexManager::new();
        store
            .create_index(&mut mgr, "notes", "f_alice_0", CreateIndexKind::Fts)
            .expect("create fts index");
        mgr
    }

    #[test]
    fn put_record_indexed_keeps_fts_in_sync_on_insert_and_update() {
        let mut s = store();
        let mgr = notes_fts(&s);
        // Insert: searchable immediately.
        s.put_record_indexed(&note("n1", "offline rebuild keeps indexes honest"), &mgr)
            .unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n1".to_string()]
        );
        // Overwrite the same id with new text: old term gone, new term present,
        // no duplicate rows.
        s.put_record_indexed(&note("n1", "lunch plans for the team"), &mgr)
            .unwrap();
        assert!(mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap()
            .is_empty());
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "lunch").unwrap(),
            vec!["n1".to_string()]
        );
    }

    #[test]
    fn patch_record_indexed_runs_fts_sync_without_corrupting_the_row() {
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        // Patch a display field. The FTS index mirrors the stable
        // `field_ids.f_alice_0` (DL-7), which `patch` does not touch, so the
        // searchable value is unchanged — but the sync still runs and must leave
        // exactly one consistent row (no duplicate, no loss).
        let env = s
            .patch_record_indexed(
                "notes",
                "n1",
                &fields(&[("tag", serde_json::json!("data"))])
                    .into_iter()
                    .collect(),
                None,
                &mgr,
            )
            .unwrap();
        assert_eq!(env.fields.get("tag"), Some(&serde_json::json!("data")));
        let hits = mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap();
        assert_eq!(
            hits,
            vec!["n1".to_string()],
            "exactly one FTS row mirrors the canonical field_id after patch"
        );
    }

    #[test]
    fn put_record_indexed_with_changed_field_id_updates_fts() {
        // When the stable field_id value itself changes (the case FTS truly
        // tracks), put_record_indexed re-mirrors it.
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        s.put_record_indexed(&note("n1", "lunch plans"), &mgr).unwrap();
        assert!(mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap()
            .is_empty());
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "lunch").unwrap(),
            vec!["n1".to_string()]
        );
    }

    #[test]
    fn delete_record_indexed_drops_record_from_fts() {
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        s.put_record_indexed(&note("n2", "offline sync path"), &mgr).unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n1".to_string(), "n2".to_string()]
        );
        // Tombstone n1: it drops out of FTS; n2 still matches.
        let env = s.delete_record_indexed("notes", "n1", None, &mgr).unwrap();
        assert!(env.deleted);
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n2".to_string()]
        );
    }

    /// Review 041/042 finding 3: the applet-facing DL-17 mutation path
    /// (`apply_mutation` / `transact_mutations`) must keep active FTS shadow
    /// tables in sync in the same transaction. Build FTS, insert/patch a matching
    /// record through the mutation surface, then query WITHOUT a manual rebuild —
    /// the record is found because the mutation refreshed FTS atomically.
    #[test]
    fn apply_mutation_keeps_active_fts_in_sync_without_rebuild() {
        let mut s = store();
        // FTS index over the stable id the display field `body` MATERIALIZES to
        // through the mutation surface (`f_body`), so a record inserted purely via
        // the DL-17 path — with NO pre-seeded field_ids and NO manual rebuild — is
        // searchable. This is the mutation-discriminating coverage review 045/046
        // finding 1 asks for: the mutation must materialize the stable field id
        // (not leave it empty), or the FTS row is never created.
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "notes", "f_body", CreateIndexKind::Fts)
            .expect("create fts index over the materialized stable id");

        // Insert through the applet mutation surface ONLY — no put_record pre-seed.
        let insert = Mutation::Insert {
            collection: "notes".into(),
            id: Some("n1".into()),
            fields: serde_json::json!({"body": "offline rebuild keeps indexes honest"})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(1),
        };
        s.apply_mutation(&insert, &mgr).unwrap();

        // The inserted record carries the materialized stable id...
        let stored = s.get_record("notes", "n1").unwrap().unwrap();
        assert_eq!(
            stored.field_ids.get("f_body"),
            Some(&serde_json::json!("offline rebuild keeps indexes honest")),
            "insert must materialize the stable field_id the index reads"
        );
        // ...and is FTS-visible WITHOUT a rebuild, because the insert synced FTS.
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_body", "offline").unwrap(),
            vec!["n1".to_string()],
            "mutation insert must populate active FTS without a rebuild"
        );

        // A patch that changes the indexed display field re-mirrors FTS in the
        // same transaction: the old term drops, the new term matches.
        let patch = Mutation::Patch {
            collection: "notes".into(),
            id: "n1".into(),
            fields: serde_json::json!({"body": "lunch plans for the team"})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(2),
        };
        s.apply_mutation(&patch, &mgr).unwrap();
        assert!(
            mgr.fts_match(s.connection(), "notes", "f_body", "offline").unwrap().is_empty(),
            "patch must drop the stale term from active FTS"
        );
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_body", "lunch").unwrap(),
            vec!["n1".to_string()],
            "patch must re-mirror the new term into active FTS without a rebuild"
        );

        // A transact group is equally synced: tombstoning n1 drops it from FTS.
        let del = vec![Mutation::Delete {
            collection: "notes".into(),
            id: "n1".into(),
            logical_at: Some(3),
        }];
        s.transact_mutations(&del, &mgr).unwrap();
        assert!(
            mgr.fts_match(s.connection(), "notes", "f_body", "lunch").unwrap().is_empty(),
            "transact delete must drop the record from active FTS"
        );
    }

    #[test]
    fn mutation_preserves_existing_schema_field_ids_and_index_visibility() {
        // Review 049: a display-name mutation must NOT clobber a record's
        // schema-minted stable ids. Seed a record carrying `f_alice_0` and an FTS
        // index keyed on it; a patch of an UNRELATED display field must keep
        // `f_alice_0` present AND keep the record FTS-visible (the active-FTS sync
        // deletes-then-reinserts reading `$.field_ids.f_alice_0`, so dropping that
        // id would strand the record from search).
        let mut s = store();
        let mut mgr = IndexManager::new();

        // Seed a record whose field_ids use a SCHEMA-minted id (not the f_<name>
        // stand-in the mutation surface would mint).
        let mut env = RecordEnvelope::new(
            forge_domain::CollectionId::new("notes"),
            forge_domain::RecordId::new("n1"),
            [("title".to_string(), serde_json::json!("alpha beta gamma"))]
                .into_iter()
                .collect(),
            forge_domain::LogicalTimestamp(1),
        );
        env.field_ids.insert("f_alice_0".into(), serde_json::json!("alpha beta gamma"));
        s.put_record(&env).unwrap();

        // FTS index over the schema id, populated from the canonical record.
        s.create_index(&mut mgr, "notes", "f_alice_0", CreateIndexKind::Fts)
            .expect("create fts index on the schema-minted id");
        s.build_indexes(&mgr).unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "beta").unwrap(),
            vec!["n1".to_string()],
            "baseline: record is FTS-visible on its schema id"
        );

        // Patch an UNRELATED display field through the DL-17 mutation surface.
        let patch = Mutation::Patch {
            collection: "notes".into(),
            id: "n1".into(),
            fields: serde_json::json!({"tag": "urgent"}).as_object().unwrap().clone(),
            logical_at: Some(2),
        };
        s.apply_mutation(&patch, &mgr).unwrap();

        let stored = s.get_record("notes", "n1").unwrap().unwrap();
        // The schema id SURVIVES the mutation (the bug clobbered it to f_title/f_tag).
        assert_eq!(
            stored.field_ids.get("f_alice_0"),
            Some(&serde_json::json!("alpha beta gamma")),
            "schema-minted f_alice_0 must survive a display-name patch (review 049)"
        );
        // And the record is STILL FTS-visible on the schema id after the patch.
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "beta").unwrap(),
            vec!["n1".to_string()],
            "record must remain FTS-visible on its schema id after an unrelated patch"
        );
        // The new display field also got its stand-in (so it is itself indexable).
        assert_eq!(
            stored.field_ids.get("f_tag"),
            Some(&serde_json::json!("urgent")),
            "the patched display field still materializes its own stand-in id"
        );
    }

    /// Review 041/042 finding 4: text_search composes with the rest of the
    /// pipeline. The FTS MATCH set is filtered by `where`, reduced by `aggregate`,
    /// and bounded by rank-path `limit` — none of which the early-return path used
    /// to honor.
    /// Build a note with a `tag` display field (so a text search can compose
    /// with a `tag` filter), populating the FTS-mirrored `field_ids.f_alice_0`.
    fn tagged_note(id: &str, body: &str, tag: &str) -> RecordEnvelope {
        let mut env = note(id, body);
        env.fields.insert("tag".into(), serde_json::json!(tag));
        env
    }

    #[test]
    fn text_search_composes_with_filter_aggregate_and_limit() {
        let mut s = store();
        let mgr = notes_fts(&s);
        // Three notes match "offline"; tag distinguishes two buckets.
        s.put_record_indexed(&tagged_note("n1", "offline rebuild keeps indexes honest", "data"), &mgr).unwrap();
        s.put_record_indexed(&tagged_note("n2", "lunch plans for the team", "personal"), &mgr).unwrap(); // no "offline"
        s.put_record_indexed(&tagged_note("n3", "offline sync path for the data plane", "data"), &mgr).unwrap();
        s.put_record_indexed(&tagged_note("n4", "offline indexing notes", "personal"), &mgr).unwrap();

        // text_search + filter: "offline" matches n1,n3,n4; tag=data keeps n1,n3.
        let q_filter = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"}
        }))
        .unwrap();
        let planned_filter = s.query_planned(&q_filter, &mgr).unwrap();
        let mut ids = planned_filter.ids();
        ids.sort();
        assert_eq!(ids, vec!["n1", "n3"], "text search must compose with the where filter");
        // Review 045/046 finding 2: the FTS MATCH serves the search (uses_index),
        // but the `tag` filter is over an UNINDEXED field, so the planner must NOT
        // suppress the full_scan warning just because a text search is present.
        assert!(planned_filter.uses_index, "the FTS match still uses its index");
        assert!(
            planned_filter.warnings.iter().any(|w| {
                w.code == "planner.full_scan"
                    && w.reason == FullScanReason::NoIndex
                    && w.field_name.as_deref() == Some("tag")
            }),
            "text_search must not mask the uncovered `tag` filter scan: {:?}",
            planned_filter.warnings
        );

        // text_search + aggregate(count): the FTS-matched, filtered set is 2.
        let q_count = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"},
            "aggregate": {"op": "count"}
        }))
        .unwrap();
        match s.query_planned(&q_count, &mgr).unwrap().result {
            QueryResult::Aggregate(a) => assert_eq!(a.count, Some(2), "aggregate over the match set"),
            other => panic!("expected aggregate, got {other:?}"),
        }

        // text_search + rank-path limit: limit bounds the rank-ordered result
        // (previously dropped on the rank path).
        let q_limit = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "order_by": [{"field": "rank", "dir": "asc"}],
            "limit": 1
        }))
        .unwrap();
        assert_eq!(
            s.query_planned(&q_limit, &mgr).unwrap().ids().len(),
            1,
            "rank-path limit must bound the result"
        );
    }

    #[test]
    fn text_search_without_active_fts_falls_back_and_still_composes() {
        // No FTS index registered: the portable scan finds the match set and the
        // filter/limit pipeline still composes (records are canonical), with a
        // fts_not_available warning surfaced.
        let s = store();
        let empty = IndexManager::new();
        s.put_record(&tagged_note("n1", "offline rebuild", "data")).unwrap();
        s.put_record(&tagged_note("n2", "offline lunch", "personal")).unwrap();
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"}
        }))
        .unwrap();
        let planned = s.query_planned(&q, &empty).unwrap();
        assert!(!planned.uses_index, "no active FTS -> fallback scan");
        assert_eq!(planned.warnings[0].reason, FullScanReason::FtsNotAvailable);
        assert_eq!(planned.ids(), vec!["n1"], "fallback still composes the filter");
    }

    #[test]
    fn store_create_index_builds_value_index_from_existing_records() {
        // DL-6: create after records exist -> the planner can use it.
        let s = store();
        let mut env = sample_record("tasks", "t1", "Ship");
        env.field_ids.insert("f_alice_1".into(), serde_json::json!("open"));
        s.put_record(&env).unwrap();
        let mut mgr = IndexManager::new();
        let id = s
            .create_index(&mut mgr, "tasks", "f_alice_1", CreateIndexKind::Value)
            .unwrap();
        assert_eq!(id, "idx_records_tasks_f_alice_1");
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_alice_1", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let planned = s.query_planned(&q, &mgr).unwrap();
        assert!(planned.uses_index);
        assert_eq!(planned.index_id.as_deref(), Some("idx_records_tasks_f_alice_1"));
    }
}
