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
//!
//! This crate root is a lean re-export facade (`/simplify` #7): the [`Store`]
//! handle and its accessors live in per-concern modules — `store` (open/
//! connection/transact), `kv`, `records`, `records_indexed`, `mutations`,
//! `oplog`, `crdt`, `runs`, and `query_exec` — each contributing an `impl Store`
//! block. The public surface re-exported here is byte-stable so `forge-core`,
//! `forge-runtime`, and `forge-sync` compile unchanged.

// `export`'s test module reaches `crate::RecordEnvelope`; re-export it
// crate-internally (test-only) so that path stays stable after the lib.rs split
// (/simplify #7). Not part of the public API.
#[cfg(test)]
pub(crate) use forge_domain::RecordEnvelope;

mod errors;
pub(crate) use errors::*;

pub mod query;

pub use query::{
    compile_select, AggregateResult, CompiledSelect, Dir, FieldRef, Filter, FullScanReason,
    GroupResult, Mutation, Op, OrderBy, PlannedQuery, PlannerWarning, Predicate, Query, QueryResult,
    QueryRow, TextSearch,
};

pub mod index;
pub use index::{CreateIndexKind, IndexDef, IndexKind, IndexManager, IndexState};

pub mod crdt_write;
pub use crdt_write::{collection_doc_id, collection_of_doc, RemoteChunk, CHUNK_FORMAT, LOCAL_PEER_ID};

pub mod compaction;
pub use compaction::{CompactionOptions, CompactionReport, CompactionSafeHorizon};

pub mod export;
pub use export::{
    bundle_meta, is_local_only_namespace, ExportOptions, RunLogPolicy, EXPORT_FORMAT_VERSION,
    STORAGE_SCHEMA_VERSION,
};

// --- Per-concern Store modules (each adds an `impl Store` block) ----------

mod store;
pub use store::Store;

/// The open SQLite transaction handle [`Store::transact`] hands its closure, and
/// the receiver type of the `*_tx` helpers (`kv_set_tx`, …). Re-exported so a
/// caller composing several tx-scoped writes in one `transact` closure (e.g. the
/// core's CR-7 lifecycle commit) can name it without a direct `rusqlite`
/// dependency.
pub use rusqlite::Transaction;

mod kv;
pub use kv::{kv_delete_tx, kv_get_tx, kv_set_tx};
mod mutations;
mod query_exec;
mod records;
pub use records::{get_record_tx, put_record_tx};
mod records_indexed;
mod runs;

mod oplog;
pub use oplog::OpRow;

mod crdt;
pub use crdt::{ChunkRow, SnapshotRow};

// Crate-internal helpers other storage modules (`crdt_write`, `compaction`)
// reach as `crate::<name>`. Re-exported here so those paths stay stable after
// the lib.rs split (/simplify #7); not part of the public API.
pub(crate) use records::{bump_updated_at, materialize_field_ids};
pub(crate) use store::now_ms;

#[cfg(test)]
mod tests;
