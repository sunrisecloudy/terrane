//! DL-19 compaction + DL-21 tombstone GC for the CRDT substrate.
//!
//! `forge-storage` does not own peer membership or transport state, so the caller
//! supplies the safe horizon: the oldest frontier every still-tracked peer has
//! acknowledged. This module only enforces that horizon while rewriting local
//! history into compact Loro snapshot chunks.

use crate::crdt_write::rebuild_projection_tx;
use crate::index::IndexManager;
use crate::{map_json, map_sql, now_ms, Store, CHUNK_FORMAT};
use forge_crdt::{CrdtDoc, RecordsDoc};
use forge_domain::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

/// Caller-supplied horizon for safe history compaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CompactionSafeHorizon {
    /// Safest default: retain all history.
    #[default]
    RetainAll,
    /// No tracked peer needs incremental history, so each doc may compact through
    /// its latest local chunk.
    AllPeersAcked,
    /// Per-doc oldest acknowledged chunk sequence among still-tracked peers.
    /// Missing docs default to `0` and are not compacted.
    Frontiers(BTreeMap<String, u64>),
}

impl CompactionSafeHorizon {
    /// Convenience constructor for a single doc frontier.
    pub fn from_doc_frontier(doc_id: impl Into<String>, frontier: u64) -> Self {
        let mut map = BTreeMap::new();
        map.insert(doc_id.into(), frontier);
        Self::Frontiers(map)
    }

    fn compact_to(&self, doc_id: &str, latest: u64) -> u64 {
        match self {
            Self::RetainAll => 0,
            Self::AllPeersAcked => latest,
            Self::Frontiers(frontiers) => frontiers.get(doc_id).copied().unwrap_or(0).min(latest),
        }
    }
}

/// The DL-20 default change-feed retention window: ~90 days, expressed as a count
/// of logical versions (the per-doc chunk frontier IS the logical clock of the
/// chunk stream — a logical clock, never a wall clock, so retention stays
/// replay-deterministic). The spec defaults to 90 days "configurable"; M0a models
/// the window as logical versions and the caller supplies the configured value.
pub const DEFAULT_RETENTION_WINDOW: u64 = 90;

/// DL-20 change-feed retention: the window of recent history the per-record change
/// feed (the oplog rows powering undo/audit) must NOT be pruned within, even when
/// the DL-19 safe horizon would otherwise fold those chunks away.
///
/// The window is a count of LOGICAL VERSIONS (the per-doc chunk frontier, e.g.
/// `chunk-0007 → 7`), NOT a wall-clock duration — the replayable path uses only the
/// logical clock, so retention is deterministic (the SC-12/audit determinism
/// lesson). Compaction is handed the externally-supplied `now_version` (the current
/// frontier, a logical clock); a chunk/oplog row whose frontier is within
/// `[now_version - window, now_version]` is PROTECTED from pruning. Entries beyond
/// the window may be pruned (subject also to the DL-19 peer safe horizon — both
/// floors apply, the lower one wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// How many recent logical versions of change-feed/oplog history to protect from
    /// pruning. Defaults to [`DEFAULT_RETENTION_WINDOW`] (~90 days).
    pub window: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            window: DEFAULT_RETENTION_WINDOW,
        }
    }
}

impl RetentionPolicy {
    /// A policy with an explicit window (count of logical versions).
    pub fn new(window: u64) -> Self {
        Self { window }
    }

    /// The oldest version still PROTECTED by this policy given the current logical
    /// frontier `now_version`: a chunk/oplog entry at a frontier `>= protected_floor`
    /// must not be pruned. With `now_version <= window` everything is protected
    /// (floor `0`). Saturating so an early-life workspace never underflows.
    fn protected_floor(&self, now_version: u64) -> u64 {
        now_version.saturating_sub(self.window).saturating_add(1)
    }
}

/// Options for [`Store::compact_history`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionOptions {
    /// The safe horizon. Defaults to retain-all so callers must opt in.
    pub safe_horizon: CompactionSafeHorizon,
    /// Explicitly compact through latest local history even when a tracked peer
    /// has not acknowledged it. This models the "peer reset / full-state resync"
    /// opt-in from DL-19.
    pub allow_peer_reset: bool,
    /// DL-20 change-feed retention: when set, the most-recent `window` logical
    /// versions of change-feed/oplog history are PROTECTED from pruning even when
    /// the safe horizon (and `allow_peer_reset`) would otherwise fold them away.
    /// `None` keeps the prior DL-19-only behavior (no retention floor). The window
    /// is a logical-version count — no wall clock — so compaction stays
    /// replay-deterministic.
    pub retention: Option<RetentionPolicy>,
}

impl CompactionOptions {
    pub fn all_peers_acked() -> Self {
        Self {
            safe_horizon: CompactionSafeHorizon::AllPeersAcked,
            allow_peer_reset: false,
            retention: None,
        }
    }

    pub fn with_frontiers(frontiers: BTreeMap<String, u64>) -> Self {
        Self {
            safe_horizon: CompactionSafeHorizon::Frontiers(frontiers),
            allow_peer_reset: false,
            retention: None,
        }
    }

    /// Attach a DL-20 retention policy, returning `self` for builder-style use. The
    /// most-recent `policy.window` logical versions of the change feed are then
    /// protected from pruning regardless of the safe horizon.
    pub fn with_retention(mut self, policy: RetentionPolicy) -> Self {
        self.retention = Some(policy);
        self
    }
}

/// Summary of one compaction run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub docs_examined: usize,
    pub docs_compacted: usize,
    pub snapshot_chunks_written: usize,
    pub chunks_removed: usize,
    pub oplog_rows_removed: usize,
    /// Number of `record.delete` oplog rows folded into a compact snapshot. This
    /// is the DL-21 tombstone-GC signal: the delete tombstone remains represented
    /// by the Loro snapshot's frontier, but the standalone delete chunk/op row is
    /// gone once past the safe horizon.
    pub tombstones_compacted: usize,
}

#[derive(Debug, Clone)]
struct ChunkForCompaction {
    chunk_id: String,
    format: String,
    payload: Vec<u8>,
    frontier: Option<u64>,
}

impl Store {
    /// Compact CRDT history in one transaction without changing the materialized
    /// projection.
    ///
    /// For each `doc_id`, chunks at or below the supplied safe horizon are folded
    /// into one compact Loro snapshot chunk (`compact-NNNN`). Chunks above the
    /// horizon remain as incremental history so a tracked peer at the frontier
    /// can still receive and converge from the missing suffix. When
    /// `allow_peer_reset` is true, the horizon is treated as the latest local
    /// chunk for every doc; callers must then serve older peers via full-state
    /// resync rather than incremental chunks.
    ///
    /// The method validates DL-19 by snapshotting the projection before the
    /// rewrite, rebuilding the projection from compacted history inside the same
    /// transaction, and rolling back if the rebuilt projection differs.
    pub fn compact_history(
        &mut self,
        opts: &CompactionOptions,
        indexes: &IndexManager,
    ) -> Result<CompactionReport> {
        let peer_id = self.crdt_peer_id();
        let opts = opts.clone();
        self.transact(|tx| {
            let before = projection_snapshot_tx(tx)?;
            let doc_ids = list_doc_ids_tx(tx)?;
            let mut report = CompactionReport {
                docs_examined: doc_ids.len(),
                ..CompactionReport::default()
            };
            for doc_id in doc_ids {
                compact_doc_tx(tx, &doc_id, peer_id, &opts, &mut report)?;
            }
            rebuild_projection_tx(tx, peer_id, indexes)?;
            let after = projection_snapshot_tx(tx)?;
            if before != after {
                return Err(CoreError::StorageError(
                    "compaction changed the materialized projection; rolled back".into(),
                ));
            }
            Ok(report)
        })
    }
}

fn compact_doc_tx(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    peer_id: u64,
    opts: &CompactionOptions,
    report: &mut CompactionReport,
) -> Result<()> {
    let chunks = list_chunks_tx(tx, doc_id)?;
    if chunks.is_empty() {
        return Ok(());
    }
    let latest = chunks.iter().filter_map(|c| c.frontier).max().unwrap_or(0);
    if latest == 0 {
        return Ok(());
    }
    let mut compact_to = if opts.allow_peer_reset {
        latest
    } else {
        opts.safe_horizon.compact_to(doc_id, latest)
    };
    // DL-20 retention: never fold/prune change-feed history within the configured
    // window. The window is a logical-version count off the current frontier
    // (`latest`), so the protected floor is deterministic (no wall clock). Clamp the
    // compaction boundary to just BELOW the protected floor: a chunk at or above the
    // floor keeps its standalone oplog row (the change feed for the last `window`
    // versions stays intact and powers undo/audit). Both floors apply — the DL-19
    // safe horizon and the DL-20 retention floor — and the lower boundary wins.
    if let Some(retention) = opts.retention {
        let protected_floor = retention.protected_floor(latest);
        // Versions strictly below the floor may still compact; the floor itself and
        // everything newer is retained, so cap `compact_to` at `floor - 1`.
        let retention_cap = protected_floor.saturating_sub(1);
        compact_to = compact_to.min(retention_cap);
    }
    if compact_to == 0 {
        return Ok(());
    }

    let compact_id = compact_chunk_id(compact_to);
    let foldable: Vec<&ChunkForCompaction> = chunks
        .iter()
        .filter(|c| c.frontier.is_some_and(|frontier| frontier <= compact_to))
        .collect();
    let removable: Vec<&ChunkForCompaction> = foldable
        .iter()
        .copied()
        .filter(|c| c.chunk_id != compact_id)
        .collect();
    if foldable.is_empty() || removable.is_empty() {
        return Ok(());
    }

    let refs: Vec<&[u8]> = foldable.iter().map(|c| c.payload.as_slice()).collect();
    let doc = RecordsDoc::from_updates(peer_id, &refs)?;
    let snapshot = doc.export_snapshot()?;

    let compact_exists = tx
        .query_row(
            "SELECT 1 FROM crdt_chunks WHERE doc_id = ?1 AND chunk_id = ?2",
            params![doc_id, compact_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(map_sql)?
        .is_some();
    if !compact_exists {
        tx.execute(
            "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc_id, compact_id, CHUNK_FORMAT, snapshot, now_ms()],
        )
        .map_err(map_sql)?;
        report.snapshot_chunks_written += 1;
    }

    let removable_ids: BTreeSet<String> = removable.iter().map(|c| c.chunk_id.clone()).collect();
    let removed_delete_ops = count_delete_ops_for_chunks(tx, doc_id, &removable_ids)?;
    for chunk_id in &removable_ids {
        tx.execute(
            "DELETE FROM crdt_chunks WHERE doc_id = ?1 AND chunk_id = ?2",
            params![doc_id, chunk_id],
        )
        .map_err(map_sql)?;
        let op_id = format!("{doc_id}#{chunk_id}");
        let removed = tx
            .execute("DELETE FROM oplog WHERE op_id = ?1", params![op_id])
            .map_err(map_sql)?;
        report.oplog_rows_removed += removed;
    }

    insert_compaction_op_tx(tx, doc_id, &compact_id, compact_to, &removable_ids)?;
    report.docs_compacted += 1;
    report.chunks_removed += removable_ids.len();
    report.tombstones_compacted += removed_delete_ops;
    Ok(())
}

fn list_doc_ids_tx(tx: &rusqlite::Transaction<'_>) -> Result<Vec<String>> {
    let mut stmt = tx
        .prepare("SELECT DISTINCT doc_id FROM crdt_chunks ORDER BY doc_id")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_sql)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(map_sql)?);
    }
    Ok(out)
}

fn list_chunks_tx(tx: &rusqlite::Transaction<'_>, doc_id: &str) -> Result<Vec<ChunkForCompaction>> {
    let mut stmt = tx
        .prepare(
            "SELECT chunk_id, format, payload FROM crdt_chunks
              WHERE doc_id = ?1 ORDER BY created_at, chunk_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(params![doc_id], |row| {
            let chunk_id: String = row.get(0)?;
            Ok(ChunkForCompaction {
                frontier: chunk_frontier(&chunk_id),
                chunk_id,
                format: row.get(1)?,
                payload: row.get(2)?,
            })
        })
        .map_err(map_sql)?;
    let mut out = Vec::new();
    for row in rows {
        let chunk = row.map_err(map_sql)?;
        if chunk.format == CHUNK_FORMAT {
            out.push(chunk);
        }
    }
    Ok(out)
}

fn projection_snapshot_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<BTreeMap<(String, String), String>> {
    let mut stmt = tx
        .prepare("SELECT collection, id, data FROM records ORDER BY collection, id")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(map_sql)?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (collection, id, data) = row.map_err(map_sql)?;
        out.insert((collection, id), data);
    }
    Ok(out)
}

fn count_delete_ops_for_chunks(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    chunk_ids: &BTreeSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    for chunk_id in chunk_ids {
        let op_id = format!("{doc_id}#{chunk_id}");
        let is_delete = tx
            .query_row(
                "SELECT kind FROM oplog WHERE op_id = ?1",
                params![op_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sql)?
            .is_some_and(|kind| kind == "record.delete");
        if is_delete {
            count += 1;
        }
    }
    Ok(count)
}

fn insert_compaction_op_tx(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    compact_id: &str,
    compact_to: u64,
    removed: &BTreeSet<String>,
) -> Result<()> {
    let op_id = format!("{doc_id}#{compact_id}");
    let payload = serde_json::to_vec(&serde_json::json!({
        "doc_id": doc_id,
        "chunk_id": compact_id,
        "kind": "history.compact",
        "compact_to": compact_to,
        "removed_chunks": removed.iter().collect::<Vec<_>>(),
    }))
    .map_err(|e| map_json("compaction oplog payload encode", e))?;
    tx.execute(
        "INSERT OR IGNORE INTO oplog
             (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
         VALUES (?1, 'local', 'local', ?2, 'history.compact', ?3, ?4)",
        params![op_id, compact_to as i64, payload, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

fn chunk_frontier(chunk_id: &str) -> Option<u64> {
    chunk_id
        .strip_prefix("chunk-")
        .or_else(|| chunk_id.strip_prefix("compact-"))
        .and_then(|n| n.parse::<u64>().ok())
}

fn compact_chunk_id(frontier: u64) -> String {
    format!("compact-{frontier:04}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{collection_doc_id, IndexManager, Mutation, RemoteChunk};
    use serde_json::json;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("object").clone()
    }

    fn insert(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Insert {
            collection: collection.into(),
            id: Some(id.into()),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn patch(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Patch {
            collection: collection.into(),
            id: id.into(),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn delete(collection: &str, id: &str, at: i64) -> Mutation {
        Mutation::Delete {
            collection: collection.into(),
            id: id.into(),
            logical_at: Some(at),
        }
    }

    fn projection_snapshot(s: &Store) -> BTreeMap<(String, String), serde_json::Value> {
        let mut stmt = s
            .connection()
            .prepare("SELECT collection, id, data FROM records ORDER BY collection, id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        let mut out = BTreeMap::new();
        for row in rows {
            let (collection, id, data) = row.unwrap();
            out.insert((collection, id), serde_json::from_str(&data).unwrap());
        }
        out
    }

    fn substrate_snapshot(s: &Store) -> (Vec<String>, Vec<String>) {
        let chunks = s
            .get_chunks(&collection_doc_id("tasks"))
            .unwrap()
            .into_iter()
            .map(|c| c.chunk_id)
            .collect::<Vec<_>>();
        let ops = s
            .list_ops()
            .unwrap()
            .into_iter()
            .map(|op| op.op_id)
            .collect::<Vec<_>>();
        (chunks, ops)
    }

    fn remote_chunks_after(s: &Store, doc_id: &str, frontier: u64) -> Vec<RemoteChunk> {
        s.get_chunks(doc_id)
            .unwrap()
            .into_iter()
            .filter(|row| chunk_frontier(&row.chunk_id).is_some_and(|n| n > frontier))
            .map(|row| RemoteChunk {
                doc_id: doc_id.to_string(),
                chunk_id: row.chunk_id,
                format: row.format,
                payload: row.payload,
                author_actor_id: None,
                record_ids: Vec::new(),
                schema_version: None,
                registry_collection: None,
            })
            .collect()
    }

    #[test]
    fn compact_superseded_lww_chunks_keeps_projection_byte_identical() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"done": true}), 3), &idx)
            .unwrap();
        let before = projection_snapshot(&s);

        let report = s
            .compact_history(&CompactionOptions::all_peers_acked(), &idx)
            .unwrap();

        assert_eq!(projection_snapshot(&s), before);
        assert_eq!(report.docs_compacted, 1);
        assert_eq!(report.snapshot_chunks_written, 1);
        assert_eq!(report.chunks_removed, 3);
        assert_eq!(
            substrate_snapshot(&s).0,
            vec!["compact-0003".to_string()],
            "history folded into one compact snapshot chunk"
        );

        s.rebuild_projection(&idx).unwrap();
        assert_eq!(
            projection_snapshot(&s),
            before,
            "rebuild after compaction is identical"
        );

        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"done": false}), 4), &idx)
            .unwrap();
        assert!(
            s.get_chunk(&collection_doc_id("tasks"), "chunk-0004")
                .unwrap()
                .is_some(),
            "new local chunks continue after the compacted frontier"
        );
    }

    #[test]
    fn tombstone_gc_after_delete_does_not_resurrect_from_old_chunk() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        let old_insert = s.get_chunks(&collection_doc_id("tasks")).unwrap()[0].clone();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx)
            .unwrap();

        let report = s
            .compact_history(&CompactionOptions::all_peers_acked(), &idx)
            .unwrap();

        assert_eq!(report.tombstones_compacted, 1);
        assert!(s.get_record("tasks", "t1").unwrap().is_none());

        let stale = RemoteChunk {
            doc_id: collection_doc_id("tasks"),
            chunk_id: "stale-insert".into(),
            format: old_insert.format,
            payload: old_insert.payload,
            author_actor_id: Some("peer:old".into()),
            record_ids: vec!["t1".into()],
            schema_version: None,
            registry_collection: None,
        };
        s.apply_remote_chunks(&[stale], "peer:relay", &idx).unwrap();
        assert!(
            s.get_record("tasks", "t1").unwrap().is_none(),
            "post-delete compact snapshot must dominate an old insert update"
        );
    }

    #[test]
    fn compaction_is_idempotent() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();

        let first = s
            .compact_history(&CompactionOptions::all_peers_acked(), &idx)
            .unwrap();
        let substrate = substrate_snapshot(&s);
        let second = s
            .compact_history(&CompactionOptions::all_peers_acked(), &idx)
            .unwrap();

        assert_eq!(first.chunks_removed, 2);
        assert_eq!(second.chunks_removed, 0);
        assert_eq!(second.snapshot_chunks_written, 0);
        assert_eq!(substrate_snapshot(&s), substrate);
    }

    #[test]
    fn safe_horizon_keeps_chunks_still_needed_by_tracked_peer() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"done": true}), 3), &idx)
            .unwrap();

        let opts = CompactionOptions {
            safe_horizon: CompactionSafeHorizon::from_doc_frontier(collection_doc_id("tasks"), 1),
            allow_peer_reset: false,
            retention: None,
        };
        let report = s.compact_history(&opts, &idx).unwrap();

        assert_eq!(report.chunks_removed, 1);
        assert_eq!(
            substrate_snapshot(&s).0,
            vec![
                "chunk-0002".to_string(),
                "chunk-0003".to_string(),
                "compact-0001".to_string(),
            ]
        );
    }

    #[test]
    fn compacting_empty_history_is_noop() {
        let mut s = store();
        let idx = IndexManager::new();
        let report = s
            .compact_history(&CompactionOptions::all_peers_acked(), &idx)
            .unwrap();
        assert_eq!(report, CompactionReport::default());
        assert!(projection_snapshot(&s).is_empty());
    }

    #[test]
    fn tombstone_not_acked_by_safe_horizon_is_not_collected() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx)
            .unwrap();

        let opts = CompactionOptions {
            safe_horizon: CompactionSafeHorizon::from_doc_frontier(collection_doc_id("tasks"), 1),
            allow_peer_reset: false,
            retention: None,
        };
        let report = s.compact_history(&opts, &idx).unwrap();

        assert_eq!(report.tombstones_compacted, 0);
        assert_eq!(
            substrate_snapshot(&s).0,
            vec!["chunk-0002".to_string(), "compact-0001".to_string()],
            "delete chunk remains available for the peer that has not acked it"
        );
        assert!(s.get_record("tasks", "t1").unwrap().is_none());
    }

    #[test]
    fn retention_window_keeps_within_window_change_feed_and_prunes_beyond() {
        let mut s = store();
        let idx = IndexManager::new();
        // Five versions of t1: chunk-0001..chunk-0005 (frontier 1..5).
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        for (n, at) in [("B", 2), ("C", 3), ("D", 4), ("E", 5)] {
            s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": n}), at), &idx)
                .unwrap();
        }

        // All peers acked (DL-19 would fold the whole history), BUT a DL-20 retention
        // window of 2 versions must protect the last two versions' change feed
        // (frontier 4 and 5). The current frontier is 5, so protected_floor = 4.
        let opts =
            CompactionOptions::all_peers_acked().with_retention(RetentionPolicy::new(2));
        s.compact_history(&opts, &idx).unwrap();

        // The standalone oplog rows WITHIN the window survive (the change feed for the
        // last two versions powers undo/audit), and the ones beyond it are pruned.
        let op_ids: Vec<String> = s.list_ops().unwrap().into_iter().map(|o| o.op_id).collect();
        let doc = collection_doc_id("tasks");
        assert!(
            op_ids.contains(&format!("{doc}#chunk-0004")),
            "within-window change-feed entry (v4) must be retained"
        );
        assert!(
            op_ids.contains(&format!("{doc}#chunk-0005")),
            "within-window change-feed entry (v5) must be retained"
        );
        assert!(
            !op_ids.contains(&format!("{doc}#chunk-0001")),
            "beyond-window change-feed entry (v1) may be pruned"
        );
        assert!(
            !op_ids.contains(&format!("{doc}#chunk-0002")),
            "beyond-window change-feed entry (v2) may be pruned"
        );
        // The projection is unchanged by compaction (DL-19 invariant).
        assert_eq!(
            s.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
            json!("E")
        );
    }

    #[test]
    fn retention_window_larger_than_history_protects_everything() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();

        // A window wider than all history (default 90) protects every entry: nothing
        // is folded even though all peers acked.
        let opts = CompactionOptions::all_peers_acked()
            .with_retention(RetentionPolicy::default());
        let report = s.compact_history(&opts, &idx).unwrap();
        assert_eq!(report.chunks_removed, 0, "full window protects all history");
        assert_eq!(report.oplog_rows_removed, 0);
        assert_eq!(s.list_ops().unwrap().len(), 2);
    }

    #[test]
    fn peer_at_tracked_frontier_converges_after_compaction() {
        let mut src = store();
        let mut peer = store();
        let idx = IndexManager::new();
        let doc_id = collection_doc_id("tasks");

        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        let first = remote_chunks_after(&src, &doc_id, 0);
        peer.apply_remote_chunks(&first, "peer:src", &idx).unwrap();
        assert_eq!(
            peer.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
            json!("A")
        );

        src.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        src.apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "C"}), 3), &idx)
            .unwrap();

        let opts = CompactionOptions {
            safe_horizon: CompactionSafeHorizon::from_doc_frontier(doc_id.clone(), 1),
            allow_peer_reset: false,
            retention: None,
        };
        src.compact_history(&opts, &idx).unwrap();

        let missing_suffix = remote_chunks_after(&src, &doc_id, 1);
        peer.apply_remote_chunks(&missing_suffix, "peer:src", &idx)
            .unwrap();
        assert_eq!(
            peer.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
            json!("B")
        );
        assert_eq!(
            peer.get_record("tasks", "t2").unwrap().unwrap().fields["title"],
            json!("C")
        );
        assert_eq!(projection_snapshot(&peer), projection_snapshot(&src));
    }
}
