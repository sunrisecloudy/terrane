//! The [`Store`] handle: open/connection, durability PRAGMAs, the M0a schema,
//! and the single-transaction (`transact`) seam (DL-4).

use forge_domain::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crdt_write::LOCAL_PEER_ID;
use crate::errors::map_sql;

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

-- SC-12 durable append-only audit log. `seq` is the workspace-local monotonic
-- ordering key (assigned from a persisted counter, not SQLite ROWID, so it
-- replays deterministically and a caller can pin the starting sequence). There
-- is NO UPDATE/DELETE path in code: rows are only ever APPENDED. `metadata` is
-- redacted canonical JSON (never a secret value or a request/response body).
CREATE TABLE IF NOT EXISTS audit_log (
    seq           INTEGER PRIMARY KEY,
    audit_id      TEXT NOT NULL UNIQUE,
    logical_time  INTEGER NOT NULL,
    producer      TEXT NOT NULL,
    action        TEXT NOT NULL,
    decision      TEXT NOT NULL,
    actor_id      TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id   TEXT,
    collection    TEXT,
    reason        TEXT NOT NULL,
    metadata      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor_id);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);
CREATE INDEX IF NOT EXISTS idx_audit_decision ON audit_log(decision);
CREATE INDEX IF NOT EXISTS idx_audit_resource_type ON audit_log(resource_type);

-- DL-22 content-addressed attachment store. An attachment is stored ONCE per
-- content hash (the primary key): putting identical bytes a second time only
-- bumps `refcount`, so the `bytes` blob — and the storage it accounts for — is
-- counted once no matter how many records reference it (dedup). `byte_len` is the
-- exact payload length, materialized so quota accounting sums it without re-reading
-- every blob. There is no in-place rewrite of `bytes` for an existing hash: the
-- hash IS the content, so a row's bytes are immutable once written.
CREATE TABLE IF NOT EXISTS attachments (
    content_hash TEXT PRIMARY KEY,
    bytes        BLOB NOT NULL,
    byte_len     INTEGER NOT NULL,
    refcount     INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER
);
"#;

/// How long a contended write waits for the SQLite writer lock before giving up
/// (review 038 finding 3). Bounds the block when two file-backed handles contend.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Wall-clock milliseconds since the Unix epoch, used for the `updated_at` /
/// `created_at` substrate columns. This is metadata only; logical ordering for
/// the deterministic spine lives in `LogicalTimestamp`/`lamport`, not here. A
/// clock before the epoch (impossible in practice) degrades to `0` rather than
/// panicking.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A handle to the workspace SQLite store.
pub struct Store {
    pub(crate) conn: Connection,
    /// The Loro peer id this store's CRDT write path mints ops under (DL-1).
    ///
    /// Defaults to [`LOCAL_PEER_ID`] — M0a is single-writer per workspace file,
    /// so one stable id is sufficient. The in-process sync seam (SS-1/SS-2,
    /// `forge-sync`) needs two stores to write under **distinct** peer ids so
    /// concurrent same-scalar edits converge to one Loro-determined LWW winner
    /// instead of reusing a peer id across concurrent writers (which Loro
    /// forbids). [`set_crdt_peer_id`](Store::set_crdt_peer_id) sets it; the
    /// rebuild path imports history regardless of this id (it only governs the
    /// identity of *future* ops).
    pub(crate) crdt_peer_id: u64,
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
        Ok(Store {
            conn,
            crdt_peer_id: LOCAL_PEER_ID,
        })
    }

    /// Set the Loro peer id this store's CRDT write path mints ops under (DL-1),
    /// returning `self` for builder-style use.
    ///
    /// The default is [`LOCAL_PEER_ID`]; a single-writer workspace never needs to
    /// change it. The in-process sync seam (`forge-sync`, SS-1/SS-2) opens two
    /// stores with **distinct** peer ids so that concurrent edits to the *same*
    /// scalar field converge to one deterministic Loro LWW winner on both peers
    /// (Loro breaks the tie by peer id, and forbids reusing a peer id across
    /// concurrent writers). Setting it only affects the identity of *future* ops;
    /// imported history (rebuild/sync) is unaffected.
    pub fn with_crdt_peer_id(mut self, peer_id: u64) -> Self {
        self.crdt_peer_id = peer_id;
        self
    }

    /// Set the Loro peer id this store mints CRDT ops under in place (see
    /// [`with_crdt_peer_id`](Store::with_crdt_peer_id) for the rationale).
    pub fn set_crdt_peer_id(&mut self, peer_id: u64) {
        self.crdt_peer_id = peer_id;
    }

    /// The Loro peer id this store mints CRDT ops under (DL-1).
    pub fn crdt_peer_id(&self) -> u64 {
        self.crdt_peer_id
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
}
