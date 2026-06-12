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
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Map any `rusqlite` failure to a stable, displayable `CoreError`.
fn map_sql(e: rusqlite::Error) -> CoreError {
    CoreError::StorageError(e.to_string())
}

/// Map a serde_json (de)serialization failure on the storage path.
fn map_json(ctx: &str, e: serde_json::Error) -> CoreError {
    CoreError::StorageError(format!("{ctx}: {e}"))
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
    pub fn save_run(&self, run: &RunRecord) -> Result<()> {
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
                let run = serde_json::from_str(&s).map_err(|e| map_json("load_run", e))?;
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
            code_hash: "sha256:abc".into(),
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
}
