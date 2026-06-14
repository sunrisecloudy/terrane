//! DL-20 file-level time travel: a per-record change feed (WHO/WHEN/WHAT) read
//! from the append-only oplog + CRDT chunk history, and a **non-destructive**
//! restore that appends a NEW version equal to a prior state — never a destructive
//! rollback, never a rewrite of prior history.
//!
//! Normative spec: `prd-merged/02-data-layer-prd.md` DL-20 + `forge/spec/time-travel.md`.
//!
//! The substrate already records the change feed: every mutation appends one
//! immutable `crdt_chunks` row (the CRDT op, the source of truth) AND one `oplog`
//! row carrying the WHO (`actor_id`/`source`), the WHAT (the op `kind` +
//! `record_ids`), and a `(lamport, op_id)` total order matching write order
//! ([`crate::crdt_write::oplog`]). A chunk's frontier — `chunk_id_lamport(chunk_id)`,
//! e.g. `chunk-0007 → 7` — is the per-doc **version**: importing the chunks with
//! frontier ≤ `v` into a fresh [`RecordsDoc`] reconstructs the record state *as of*
//! `v` (DL-6 rebuild-by-replay, bounded by the frontier). The WHEN is the
//! reconstructed envelope's `updated_at` — the externally-supplied LOGICAL
//! timestamp (`logical_at`), NOT a wall clock, so history reads + restore stay
//! replay-deterministic (the SC-12/audit-log determinism lesson).
//!
//! RESTORE is non-destructive (DL-20): [`Store::restore_record`] reconstructs the
//! record at `to_version`, then writes it as a NEW op on the SAME DL-4 mutation
//! path ([`Store::apply_mutation_crdt`]). The record now equals its `to_version`
//! state, but every prior chunk/oplog row — including the versions in between —
//! remains intact (append-only; history is never deleted or rewritten). Restoring
//! a tombstoned/deleted version re-creates the record as a new version.

use forge_domain::{CoreError, RecordEnvelope, Result};
use serde::{Deserialize, Serialize};

use crate::crdt_write::collection_doc_id;
use crate::index::IndexManager;
use crate::store::Store;
use crate::Mutation;

/// One entry in a record's change feed (DL-20): WHO changed it, WHEN (the
/// externally-supplied logical timestamp), WHAT kind of change, and the record's
/// full state at that point (or `None` for the delete that tombstoned it).
///
/// Entries are ordered by `version` ascending — the per-doc chunk frontier, which
/// equals the oplog `(lamport, op_id)` write order — so the feed reads
/// oldest-to-newest deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The per-doc version this entry advanced the record TO — the chunk frontier
    /// (`chunk-0007 → 7`). Pass it to [`Store::restore_record`] to bring the record
    /// back to exactly this state as a new version.
    pub version: u64,
    /// WHO authored the change: the oplog `actor_id` (a local write is `"local"`; a
    /// synced write carries the original author's peer id — review 092/101).
    pub actor: String,
    /// The chunk's original author when the change arrived via sync and the actor
    /// merely forwarded it; `None` for a locally-authored change. This is the
    /// remote-import `source` field (provenance), distinct from `actor` only on a
    /// relayed import.
    pub source: Option<String>,
    /// WHEN, as the externally-supplied LOGICAL timestamp (`logical_at`) carried on
    /// the record envelope's `updated_at`. NOT a wall clock — the replayable path
    /// uses only the logical clock, so history is deterministic. `None` for a
    /// delete (no surviving envelope to read it from).
    pub logical_at: Option<u64>,
    /// WHAT: the oplog `kind` string (`record.insert`, `record.update`,
    /// `record.patch`, `record.delete`, `schema.migration`).
    pub kind: String,
    /// The record's full envelope AS OF this version, reconstructed from the chunk
    /// history (frontier ≤ `version`). `None` when the record was tombstoned at this
    /// version (a `record.delete`, or a state where the record is absent).
    pub state: Option<RecordEnvelope>,
}

impl Store {
    /// The DL-20 per-record change feed for `collection`/`id`: an ordered list of
    /// every version that touched the record, each carrying WHO (`actor`/`source`),
    /// WHEN (`logical_at`), WHAT (`kind`), and the record's full `state` at that
    /// version. Ordered oldest-to-newest by `version` (the chunk frontier, which
    /// equals the oplog `(lamport, op_id)` total order), so the read is
    /// deterministic and replay-safe.
    ///
    /// Read from the append-only oplog rows (the WHO/WHEN/WHAT metadata) joined to
    /// the CRDT chunk history (the record state). For each oplog row whose
    /// `record_ids` names `id`, the state is reconstructed by replaying the chunks
    /// with frontier ≤ that row's version into a fresh [`RecordsDoc`] (DL-6
    /// rebuild-by-replay, bounded by the version). A `schema.migration` row that
    /// touched `id` appears too (the migration is a real change to the record).
    ///
    /// RBAC: the caller (core) must hold `db.read` on `collection`, scoped from the
    /// trusted context — this storage method is the substrate read; it does not
    /// itself gate (review 048/050).
    pub fn record_history(&self, collection: &str, id: &str) -> Result<Vec<HistoryEntry>> {
        let doc_id = collection_doc_id(collection);
        let chunks = self.get_chunks(&doc_id)?;
        // Oplog rows in deterministic (lamport, op_id) order: the WHO/WHEN/WHAT feed.
        let ops = self.list_ops()?;

        // Reconstruct each entry's state from ALL chunks with frontier ≤ that entry's
        // version — by FRONTIER order, not `get_chunks` write order (review 166). After
        // DL-19/DL-20 retention compaction a `compact-NNNN` BASE chunk is written with a
        // fresh `created_at` AFTER the retained suffix it summarizes, so `get_chunks`
        // (ordered by `created_at`) returns it LAST (`chunk-0004, chunk-0005,
        // compact-0003`). A running `created_at`-prefix accumulator would then replay
        // v4/v5 WITHOUT their compact base and report `state=None` for a live retained
        // change — silently tombstoning the retained 90-day who/when/what feed. Filtering
        // by frontier ≤ version makes the base part of every retained entry's replay set
        // regardless of when it was written. (A frontier `≤ v` set is order/duplication
        // independent — Loro dedupes by version — so the result is stable.)
        let mut entries: Vec<HistoryEntry> = Vec::new();
        for chunk in &chunks {
            let version = chunk_frontier(&chunk.chunk_id);
            // Find the oplog row for THIS chunk (op_id = "{doc_id}#{chunk_id}"). A
            // compact snapshot chunk has a `history.compact` row that names no
            // record_ids, so it never produces a feed entry — exactly right, since a
            // compaction is not a change to the record.
            let op_id = format!("{doc_id}#{}", chunk.chunk_id);
            let Some(op) = ops.iter().find(|o| o.op_id == op_id) else {
                continue;
            };
            let meta = decode_op_meta(&op.payload);
            if !meta.record_ids.iter().any(|r| r == id) {
                continue; // this change did not touch our record
            }
            // Reconstruct the record state AS OF this version: replay every chunk with
            // frontier ≤ v (so a compact base summarizing folded history is included even
            // when it was WRITTEN after this entry's chunk — the retention case).
            let replayed: Vec<Vec<u8>> = chunks
                .iter()
                .filter(|c| chunk_frontier(&c.chunk_id) <= version)
                .map(|c| c.payload.clone())
                .collect();
            let state = reconstruct_record_at(self.crdt_peer_id(), &replayed, id)?;
            entries.push(HistoryEntry {
                version,
                actor: op.actor_id.clone(),
                source: meta.source,
                // WHEN: the envelope's logical updated_at (the supplied logical_at).
                // A delete has no surviving envelope, so its logical_at is None.
                logical_at: state.as_ref().map(|e| e.updated_at.0),
                kind: op.kind.clone(),
                state,
            });
        }
        // The feed reads oldest-to-newest by VERSION. `get_chunks` write order matches
        // this for an uncompacted doc, but after compaction the compact base lands last
        // in `created_at` order; sort by `version` so the (already-skipped) base never
        // reorders the retained suffix and the feed stays monotone.
        entries.sort_by_key(|e| e.version);
        Ok(entries)
    }

    /// The record's full envelope reconstructed AS OF `version` (the chunk
    /// frontier), or `None` if the record did not exist / was tombstoned at that
    /// version. The point read behind [`record_history`](Store::record_history),
    /// exposed so a caller (the time-travel command, a restore preview) can fetch a
    /// single past state without materializing the whole feed.
    ///
    /// Replays the chunks with frontier ≤ `version` into a fresh [`RecordsDoc`] and
    /// reads `id`. Deterministic and replay-safe (no wall clock). A `version` past
    /// the latest chunk yields the current state; `0` yields the empty (pre-history)
    /// state.
    pub fn record_state_at(
        &self,
        collection: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<RecordEnvelope>> {
        let doc_id = collection_doc_id(collection);
        let chunks = self.get_chunks(&doc_id)?;
        let replayed: Vec<Vec<u8>> = chunks
            .iter()
            .filter(|c| chunk_frontier(&c.chunk_id) <= version)
            .map(|c| c.payload.clone())
            .collect();
        reconstruct_record_at(self.crdt_peer_id(), &replayed, id)
    }

    /// DL-20 **non-destructive** restore: bring `collection`/`id` back to its state
    /// AS OF `to_version` by appending a NEW version — never a destructive rollback,
    /// never a rewrite of prior history.
    ///
    /// Reconstructs the record at `to_version` ([`record_state_at`](Store::record_state_at)),
    /// then writes that state through the SAME DL-4 mutation path the live spine
    /// uses ([`apply_mutation_crdt`](Store::apply_mutation_crdt)): a fresh
    /// `record.insert` carrying the full prior envelope's display fields. The record
    /// now equals its `to_version` state, but EVERY prior chunk/oplog row —
    /// including the versions written AFTER `to_version` — remains intact
    /// (`crdt_chunks` is append-only; nothing is deleted or rewritten). Restoring a
    /// tombstoned/deleted version re-creates the record as a new live version.
    ///
    /// `restored_logical_at` is the externally-supplied LOGICAL timestamp stamped on
    /// the new version (the WHEN of the restore op) — a logical clock, NOT a wall
    /// clock, so the restore replays deterministically. Reusing the spine's
    /// `logical_at` keeps the new version's `updated_at` monotone and replay-stable.
    ///
    /// Returns the NEW version (the chunk frontier the restore appended), which the
    /// caller can hand back as the audit/undo handle.
    ///
    /// Errors with a [`CoreError::QueryError`] if `to_version` reconstructs to a
    /// state that cannot be restored — currently never (a tombstoned version is a
    /// valid no-record re-create). RBAC: the caller (core) must hold `db.write` on
    /// `collection` (scoped from the trusted context).
    pub fn restore_record(
        &mut self,
        collection: &str,
        id: &str,
        to_version: u64,
        restored_logical_at: Option<i64>,
        indexes: &IndexManager,
    ) -> Result<u64> {
        let prior = self.record_state_at(collection, id, to_version)?;
        match prior {
            Some(env) => {
                // Re-create the record from its prior DISPLAY fields. An `Insert` is a
                // full (re)create even over a tombstone (DL-21 reinsert), so this works
                // whether the record is currently live or deleted. The mutation path
                // re-materializes field_ids from the display fields, so the restored
                // record is index-visible exactly like a fresh insert. We restore the
                // display fields (what the user sees); unknown/extension fields are an
                // M0a-scope simplification noted in spec/time-travel.md.
                let fields: serde_json::Map<String, serde_json::Value> =
                    env.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                self.apply_mutation_crdt(
                    &Mutation::Insert {
                        collection: collection.to_string(),
                        id: Some(id.to_string()),
                        fields,
                        logical_at: restored_logical_at,
                    },
                    indexes,
                )?;
            }
            None => {
                // The target version is a tombstone (or pre-history): "restoring" it
                // means the record should be ABSENT. If it is currently live, append a
                // delete as the new version; if it is already absent, there is nothing
                // to do but we still must return a version handle, so we report the
                // current frontier unchanged.
                if self.get_record(collection, id)?.is_some() {
                    self.apply_mutation_crdt(
                        &Mutation::Delete {
                            collection: collection.to_string(),
                            id: id.to_string(),
                            logical_at: restored_logical_at,
                        },
                        indexes,
                    )?;
                }
            }
        }
        // The new version is the current latest frontier of the collection doc.
        self.latest_version(collection)
    }

    /// The latest version (highest chunk frontier) of `collection`'s doc, or `0` if
    /// the collection has no history yet. The "current version" handle DL-20 reads
    /// expose.
    pub fn latest_version(&self, collection: &str) -> Result<u64> {
        let doc_id = collection_doc_id(collection);
        Ok(self
            .get_chunks(&doc_id)?
            .iter()
            .map(|c| chunk_frontier(&c.chunk_id))
            .max()
            .unwrap_or(0))
    }
}

/// Reconstruct a single record's envelope from the ordered chunk payloads that
/// make up history up to some version. Imports them as a batch into a fresh
/// [`RecordsDoc`] (order/duplication independent — Loro dedupes by version) and
/// reads `id`. `None` when the record is absent at that point (never written, or
/// CRDT-deleted/tombstoned). Pure replay — deterministic, no wall clock.
fn reconstruct_record_at(
    peer_id: u64,
    chunk_payloads: &[Vec<u8>],
    id: &str,
) -> Result<Option<RecordEnvelope>> {
    use forge_crdt::RecordsDoc;
    let refs: Vec<&[u8]> = chunk_payloads.iter().map(|p| p.as_slice()).collect();
    let doc = RecordsDoc::from_updates(peer_id, &refs)?;
    match doc.get_record(id) {
        Some(value) => {
            let env: RecordEnvelope = serde_json::from_value(value)
                .map_err(|e| CoreError::StorageError(format!("time-travel envelope decode: {e}")))?;
            Ok(Some(env))
        }
        None => Ok(None),
    }
}

/// The decoded subset of an oplog row's JSON payload the change feed needs: the
/// touched `record_ids` (so we know which rows touched our record) and the optional
/// remote-import `source` (provenance). Mirrors how the sync seam reads the same
/// fields back by key (`crate::crdt_write::oplog`), so the two cannot skew.
struct OpMeta {
    record_ids: Vec<String>,
    source: Option<String>,
}

/// Decode an oplog row payload into the change-feed metadata. A row whose payload
/// is malformed or carries no `record_ids` yields an empty list (it simply produces
/// no feed entry), so a non-record op (`history.compact`) is naturally skipped.
fn decode_op_meta(payload: &[u8]) -> OpMeta {
    let value: serde_json::Value = serde_json::from_slice(payload).unwrap_or(serde_json::Value::Null);
    let record_ids = value
        .get("record_ids")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let source = value
        .get("source")
        .and_then(|s| s.as_str())
        .map(str::to_string);
    OpMeta { record_ids, source }
}

/// A chunk's frontier = its zero-padded sequence number (`chunk-0007 → 7`,
/// `compact-0003 → 3`). This is the per-doc VERSION the time-travel surface speaks.
/// Mirrors the same derivation `compaction` and the oplog lamport use, so the
/// version a feed entry reports equals the oplog `lamport` of its op. A malformed id
/// degrades to `0`.
fn chunk_frontier(chunk_id: &str) -> u64 {
    chunk_id
        .strip_prefix("chunk-")
        .or_else(|| chunk_id.strip_prefix("compact-"))
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::{collection_doc_id, IndexManager, Mutation, Store};
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

    /// History lists who/when/what in version order, with the record state at each
    /// point (DL-20 change feed shape).
    #[test]
    fn history_lists_who_when_what_in_order() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"done": true}), 3), &idx)
            .unwrap();

        let feed = s.record_history("tasks", "t1").unwrap();
        assert_eq!(feed.len(), 3, "one entry per change that touched t1");
        // Ordered oldest → newest by version (chunk frontier).
        assert_eq!(feed[0].version, 1);
        assert_eq!(feed[1].version, 2);
        assert_eq!(feed[2].version, 3);
        // WHAT: op kinds.
        assert_eq!(feed[0].kind, "record.insert");
        assert_eq!(feed[1].kind, "record.patch");
        assert_eq!(feed[2].kind, "record.patch");
        // WHO: local writes are authored by "local".
        assert!(feed.iter().all(|e| e.actor == "local"));
        // WHEN: the supplied logical_at, carried on updated_at.
        assert_eq!(feed[0].logical_at, Some(1));
        assert_eq!(feed[1].logical_at, Some(2));
        assert_eq!(feed[2].logical_at, Some(3));
        // STATE at each point.
        assert_eq!(feed[0].state.as_ref().unwrap().fields["title"], json!("A"));
        assert_eq!(feed[1].state.as_ref().unwrap().fields["title"], json!("B"));
        assert_eq!(feed[2].state.as_ref().unwrap().fields["title"], json!("B"));
        assert_eq!(feed[2].state.as_ref().unwrap().fields["done"], json!(true));
    }

    /// A record-history read is deterministic: the same store yields a byte-equal
    /// feed every call (no wall clock in the replayable path).
    #[test]
    fn history_read_is_deterministic() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();

        let a = s.record_history("tasks", "t1").unwrap();
        let b = s.record_history("tasks", "t1").unwrap();
        assert_eq!(a, b, "history read must be deterministic");
    }

    /// NON-DESTRUCTIVE restore (the DL-20 keystone): restore creates a NEW version
    /// equal to a prior state, and ALL prior versions — including the ones written
    /// after the restore target — remain in history intact.
    #[test]
    fn restore_creates_new_version_and_prior_versions_remain() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "C"}), 3), &idx)
            .unwrap();

        // Capture the chunk substrate BEFORE the restore.
        let chunks_before = s
            .get_chunks(&collection_doc_id("tasks"))
            .unwrap()
            .iter()
            .map(|c| c.chunk_id.clone())
            .collect::<Vec<_>>();
        let feed_before = s.record_history("tasks", "t1").unwrap();
        assert_eq!(feed_before.len(), 3);

        // Restore to version 1 (title "A").
        let new_version = s.restore_record("tasks", "t1", 1, Some(4), &idx).unwrap();
        assert_eq!(new_version, 4, "restore appends a NEW version (chunk-0004)");

        // The record now equals its version-1 state.
        let now = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(now.fields["title"], json!("A"));

        // NON-DESTRUCTIVE: every prior chunk is still present, plus the new one.
        let chunks_after = s
            .get_chunks(&collection_doc_id("tasks"))
            .unwrap()
            .iter()
            .map(|c| c.chunk_id.clone())
            .collect::<Vec<_>>();
        for c in &chunks_before {
            assert!(chunks_after.contains(c), "prior chunk {c} must remain after restore");
        }
        assert_eq!(chunks_after.len(), chunks_before.len() + 1);
        assert!(chunks_after.contains(&"chunk-0004".to_string()));

        // The feed now has FOUR entries; the first three are byte-identical to before
        // (prior history is never rewritten), and the fourth is the restore.
        let feed_after = s.record_history("tasks", "t1").unwrap();
        assert_eq!(feed_after.len(), 4);
        assert_eq!(&feed_after[..3], &feed_before[..], "prior history unchanged");
        assert_eq!(feed_after[3].version, 4);
        assert_eq!(feed_after[3].state.as_ref().unwrap().fields["title"], json!("A"));

        // And the intermediate versions are STILL reconstructable (B at v2, C at v3).
        assert_eq!(
            s.record_state_at("tasks", "t1", 2).unwrap().unwrap().fields["title"],
            json!("B")
        );
        assert_eq!(
            s.record_state_at("tasks", "t1", 3).unwrap().unwrap().fields["title"],
            json!("C")
        );
    }

    /// Restoring a DELETED version re-creates the record as a new live version (DL-21
    /// reinsert over a tombstone), and history retains the delete.
    #[test]
    fn restore_a_deleted_record_recreates_it() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx).unwrap();
        assert!(s.get_record("tasks", "t1").unwrap().is_none(), "deleted");

        // Restore to version 1 (before the delete) → record comes back.
        let v = s.restore_record("tasks", "t1", 1, Some(3), &idx).unwrap();
        assert_eq!(v, 3);
        let now = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(now.fields["title"], json!("A"));

        // History retains the insert, the delete, and the restore.
        let feed = s.record_history("tasks", "t1").unwrap();
        assert_eq!(feed.len(), 3);
        assert_eq!(feed[0].kind, "record.insert");
        assert_eq!(feed[1].kind, "record.delete");
        assert!(feed[1].state.is_none(), "the delete version has no surviving state");
        assert_eq!(feed[2].kind, "record.insert");
        assert_eq!(feed[2].state.as_ref().unwrap().fields["title"], json!("A"));
    }

    /// "Restoring" a tombstone state on a currently-live record removes it as a new
    /// version (the symmetric direction of restore-a-deleted-record).
    #[test]
    fn restore_to_a_tombstone_deletes_the_live_record() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx).unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Z"}), 3), &idx)
            .unwrap();
        assert!(s.get_record("tasks", "t1").unwrap().is_some(), "live again");

        // Version 2 was a tombstone; restoring it deletes the live record.
        s.restore_record("tasks", "t1", 2, Some(4), &idx).unwrap();
        assert!(s.get_record("tasks", "t1").unwrap().is_none());
        // And the resurrection at v3 is still in history.
        assert_eq!(
            s.record_state_at("tasks", "t1", 3).unwrap().unwrap().fields["title"],
            json!("Z")
        );
    }

    /// A restore is itself replay-safe: a DL-6 rebuild after a restore reproduces the
    /// restored state (the restore op is in the CRDT source of truth, not a side
    /// write).
    #[test]
    fn restore_survives_rebuild() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 2), &idx)
            .unwrap();
        s.restore_record("tasks", "t1", 1, Some(3), &idx).unwrap();

        s.rebuild_projection(&idx).unwrap();
        let rebuilt = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(rebuilt.fields["title"], json!("A"), "restored state survives rebuild");
    }

    /// History only reports versions that touched the named record — a sibling
    /// record's changes never leak into another record's feed.
    #[test]
    fn history_is_scoped_to_the_named_record() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "X"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"title": "B"}), 3), &idx)
            .unwrap();

        let t1 = s.record_history("tasks", "t1").unwrap();
        assert_eq!(t1.len(), 2, "only t1's two changes");
        assert_eq!(t1[0].version, 1);
        assert_eq!(t1[1].version, 3);
        let t2 = s.record_history("tasks", "t2").unwrap();
        assert_eq!(t2.len(), 1);
        assert_eq!(t2[0].version, 2);
    }

    /// Review 166 (P1 regression): a RETAINED within-window change-feed entry must
    /// keep its WHO/WHEN/WHAT — its `state` and `logical_at` — AFTER retention
    /// compaction folds the older suffix into a `compact-NNNN` base. Compaction writes
    /// that base with a fresh `created_at` AFTER the retained suffix, so `get_chunks`
    /// (ordered by `created_at`) returns `chunk-0004, chunk-0005, compact-0003`. A
    /// naive `created_at`-prefix replay would reconstruct v4/v5 WITHOUT the compact base
    /// and report `state=None` / `logical_at=None` — silently tombstoning live retained
    /// changes. The frontier-ordered reconstruction includes the base for every entry,
    /// so the retained feed survives compaction intact.
    #[test]
    fn retained_change_feed_survives_compaction_with_state_intact() {
        use crate::{CompactionOptions, RetentionPolicy};
        let mut s = store();
        let idx = IndexManager::new();
        for (n, title) in [(1, "A"), (2, "B"), (3, "C"), (4, "D"), (5, "E")] {
            let m = if n == 1 {
                insert("tasks", "t1", json!({ "title": title }), n)
            } else {
                patch("tasks", "t1", json!({ "title": title }), n)
            };
            s.apply_mutation_crdt(&m, &idx).unwrap();
        }

        // Compact with a 2-version retention window: v4/v5 are PROTECTED, v1/v2 may be
        // folded into a compact base (with a later `created_at`).
        let opts = CompactionOptions::all_peers_acked().with_retention(RetentionPolicy::new(2));
        s.compact_history(&opts, &idx).unwrap();

        // Sanity: the substrate is ordered like `..., compact-NNNN` LAST (created_at).
        let chunk_ids: Vec<String> = s
            .get_chunks(&collection_doc_id("tasks"))
            .unwrap()
            .iter()
            .map(|c| c.chunk_id.clone())
            .collect();
        assert!(
            chunk_ids.iter().any(|c| c.starts_with("compact-")),
            "compaction produced a compact base: {chunk_ids:?}"
        );

        // The retained within-window entries keep their title + logical_at (NOT None).
        let feed = s.record_history("tasks", "t1").unwrap();
        let v4 = feed.iter().find(|e| e.version == 4).expect("v4 retained");
        assert_eq!(v4.state.as_ref().unwrap().fields["title"], json!("D"));
        assert_eq!(v4.logical_at, Some(4), "v4 keeps its logical_at after compaction");
        let v5 = feed.iter().find(|e| e.version == 5).expect("v5 retained");
        assert_eq!(v5.state.as_ref().unwrap().fields["title"], json!("E"));
        assert_eq!(v5.logical_at, Some(5), "v5 keeps its logical_at after compaction");

        // The feed reads oldest-to-newest by version regardless of the compact base's
        // late `created_at`.
        let versions: Vec<u64> = feed.iter().map(|e| e.version).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        assert_eq!(versions, sorted, "the feed is monotone by version: {versions:?}");
    }
}
