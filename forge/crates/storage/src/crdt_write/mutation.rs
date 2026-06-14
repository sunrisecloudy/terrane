//! The DL-4 CRDT-backed record mutation path: a mutation becomes a Loro op on the
//! collection's [`RecordsDoc`], the incremental update is appended as one immutable
//! chunk + one oplog row, and the affected records are materialized into the
//! `records` projection — ALL inside ONE `Store::transact` closure (DL-4 atomicity).

use super::chunk_storage::{next_chunk_id, put_chunk_tx};
use super::crdt_encoding::{envelope_from_doc, load_doc_tx, write_envelope_to_doc};
use super::oplog::{append_op_tx, chunk_id_lamport, oplog_kind};
use super::rebuild::materialize_record_into_projection;
use crate::index::IndexManager;
use crate::{map_json, Mutation, Store};
use forge_crdt::RecordsDoc;
use forge_domain::{CollectionId, CoreError, RecordEnvelope, RecordId, Result};

/// The append-only chunk format tag written into `crdt_chunks.format`. Loro
/// incremental updates; matches the fixtures' `chunk_format: "loro"`.
pub const CHUNK_FORMAT: &str = "loro";

/// The Loro peer id every workspace-local `RecordsDoc` writes under (DL-1). M0a is
/// single-writer per workspace file (multi-peer transport is deferred — see
/// `spec/crdt-write-path.md` "Deferred"), so one stable peer id is sufficient and
/// keeps rebuilt docs writing future ops under the same identity as the maintained
/// doc. Rebuild imports history regardless of this id.
pub const LOCAL_PEER_ID: u64 = 1;

/// The CRDT `doc_id` for a collection's records document (DL-2 `collection_doc`):
/// `collection/<name>`. This is the key into `crdt_chunks` / `crdt_snapshots`.
pub fn collection_doc_id(collection: &str) -> String {
    format!("collection/{collection}")
}

/// Apply ONE leaf mutation to an in-memory `RecordsDoc`, mutating CRDT state to the
/// post-state envelope (insert/update/patch) or removing the record (delete). This
/// is pure CRDT work; persistence (chunk/oplog/projection) is the caller's job
/// inside the transaction. A `Transact` leaf is rejected here — groups are
/// flattened by the caller so each leaf shares the one doc/commit/export.
fn apply_leaf_to_doc(doc: &RecordsDoc, m: &Mutation) -> Result<()> {
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
            // Insert is a full (re)create of the visible record, even over a prior
            // tombstone for the same id (DL-21 reinsert): start from a fresh
            // envelope rather than merging the old one.
            let mut env = RecordEnvelope::new(
                CollectionId::new(collection.clone()),
                RecordId::new(id.clone()),
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                at,
            );
            crate::materialize_field_ids(&mut env);
            write_envelope_to_doc(doc, &env)
        }
        Mutation::Update {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = require_record(doc, collection, id, "update")?;
            // Update REPLACES the display fields (DL-17) but preserves
            // unknown_fields/extensions already in the envelope (DL-9).
            env.fields = fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            crate::materialize_field_ids(&mut env);
            crate::bump_updated_at(&mut env, *logical_at);
            write_envelope_to_doc(doc, &env)
        }
        Mutation::Patch {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = require_record(doc, collection, id, "patch")?;
            // Patch MERGES supplied fields, preserving omitted display fields and
            // unknown/forward-compat fields (DL-9).
            for (k, v) in fields {
                env.fields.insert(k.clone(), v.clone());
            }
            crate::materialize_field_ids(&mut env);
            crate::bump_updated_at(&mut env, *logical_at);
            write_envelope_to_doc(doc, &env)
        }
        Mutation::Delete {
            collection,
            id,
            logical_at: _,
        } => {
            // Whole-record CRDT delete (DL-4/DL-17): the record vanishes from
            // get_record/list_record_ids/materialized and the tombstone rides in
            // Loro history so it propagates and survives rebuild.
            require_record(doc, collection, id, "delete")?;
            doc.delete_record(id)
        }
        Mutation::Transact { .. } => Err(CoreError::QueryError(
            "nested transact leaf reached the CRDT applier; flatten groups first".into(),
        )),
    }
}

/// Load the current envelope for a mutation that requires the record to exist,
/// surfacing the same `QueryError` the projection-only path uses on a miss.
fn require_record(
    doc: &RecordsDoc,
    collection: &str,
    id: &str,
    verb: &str,
) -> Result<RecordEnvelope> {
    envelope_from_doc(doc, id)?.ok_or_else(|| {
        CoreError::QueryError(format!("{verb}: record {collection}/{id} does not exist"))
    })
}

/// Flatten a (possibly nested) mutation into its ordered leaves, rejecting an
/// `Insert` with no id early (so a group fails before any CRDT/SQLite work).
fn flatten_leaves<'a>(m: &'a Mutation, out: &mut Vec<&'a Mutation>) {
    match m {
        Mutation::Transact { items } => {
            for it in items {
                flatten_leaves(it, out);
            }
        }
        other => out.push(other),
    }
}

/// Collect the distinct record ids a set of leaves touches, in first-touch order,
/// so the projection materialization step only re-reads the affected records.
fn touched_ids(leaves: &[&Mutation]) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for m in leaves {
        let pair = match m {
            Mutation::Insert { collection, id, .. } => {
                id.as_ref().map(|i| (collection.clone(), i.clone()))
            }
            Mutation::Update { collection, id, .. }
            | Mutation::Patch { collection, id, .. }
            | Mutation::Delete { collection, id, .. } => {
                Some((collection.clone(), id.clone()))
            }
            Mutation::Transact { .. } => None,
        };
        if let Some(p) = pair {
            if !seen.contains(&p) {
                seen.push(p);
            }
        }
    }
    seen
}

/// Extract the collection a leaf mutation targets (the doc selector).
fn leaf_collection(m: &Mutation) -> Result<String> {
    match m {
        Mutation::Insert { collection, .. }
        | Mutation::Update { collection, .. }
        | Mutation::Patch { collection, .. }
        | Mutation::Delete { collection, .. } => Ok(collection.clone()),
        Mutation::Transact { .. } => Err(CoreError::QueryError(
            "transact leaf reached the collection selector; flatten groups first".into(),
        )),
    }
}

impl Store {
    /// The DL-4 CRDT-backed mutation path for a **single** record mutation.
    ///
    /// This is the real write path the spine routes through (replacing the
    /// projection-only [`apply_mutation`](Store::apply_mutation)): a mutation
    /// becomes a Loro op on the collection's `RecordsDoc`, the incremental update
    /// is appended to `crdt_chunks` (immutable id) plus an `oplog` row, and the
    /// affected record(s) are materialized into the `records` projection — all in
    /// ONE SQLite transaction. A failure rolls back the chunk, the op, AND the
    /// projection together (DL-4 atomicity).
    ///
    /// A `Transact` here is rejected; use
    /// [`transact_mutations_crdt`](Store::transact_mutations_crdt) for groups.
    pub fn apply_mutation_crdt(
        &mut self,
        m: &Mutation,
        indexes: &IndexManager,
    ) -> Result<()> {
        if matches!(m, Mutation::Transact { .. }) {
            return Err(CoreError::QueryError(
                "nested transact is not allowed; pass items to transact_mutations_crdt".into(),
            ));
        }
        self.write_group_crdt(std::slice::from_ref(m), oplog_kind(m), indexes)
            .map(|_| ())
    }

    /// The DL-4 CRDT-backed mutation path for a **group** of mutations (DL-17
    /// `transact`): all leaves apply to the same `RecordsDoc`, the doc is committed
    /// once and exported as a SINGLE incremental chunk, and one oplog row + the
    /// affected projection rows are written in ONE SQLite transaction. All-or-
    /// nothing: any failure rolls back the chunk, op, and projection together.
    ///
    /// Returns the number of leaf mutations applied (nested groups are flattened).
    pub fn transact_mutations_crdt(
        &mut self,
        items: &[Mutation],
        indexes: &IndexManager,
    ) -> Result<usize> {
        let group = Mutation::Transact {
            items: items.to_vec(),
        };
        self.write_group_crdt(std::slice::from_ref(&group), oplog_kind(&group), indexes)
    }

    /// Shared DL-4 engine for both the single-mutation and group paths. `top` is
    /// the original mutation(s) as the caller sees them (one leaf, or one group);
    /// `kind` is the oplog kind string for the whole logical write.
    ///
    /// Steps (all inside one `transact`): load-or-rebuild the doc from chunks,
    /// capture `before_version`, apply every flattened leaf to the doc, commit the
    /// doc, export the incremental update since `before_version`, append it as one
    /// immutable chunk, append one oplog row, then re-materialize each touched
    /// record into the `records` projection (FTS-synced) from the post-mutation
    /// CRDT state. Returns the leaf count.
    fn write_group_crdt(
        &mut self,
        top: &[Mutation],
        kind: &str,
        indexes: &IndexManager,
    ) -> Result<usize> {
        // Flatten to leaves up front and validate the doc is single-collection:
        // a `RecordsDoc` is per collection (DL-2), so a group spanning collections
        // would need multiple docs — out of scope for M0a's one-doc transact.
        let mut leaves: Vec<&Mutation> = Vec::new();
        for m in top {
            flatten_leaves(m, &mut leaves);
        }
        if leaves.is_empty() {
            return Ok(0);
        }
        let collection = leaf_collection(leaves[0])?;
        for m in &leaves {
            let c = leaf_collection(m)?;
            if c != collection {
                return Err(CoreError::QueryError(format!(
                    "a single CRDT transact group must target one collection; \
                     found both '{collection}' and '{c}'"
                )));
            }
        }
        let doc_id = collection_doc_id(&collection);
        let touched = touched_ids(&leaves);
        let leaf_count = leaves.len();
        let peer_id = self.crdt_peer_id();

        self.transact(|tx| {
            // 1-4. Load/reconstruct the doc and capture the pre-mutation version.
            let doc = load_doc_tx(tx, &doc_id, peer_id)?;
            let before = doc.version();

            // 5. Apply every leaf to the doc.
            for m in &leaves {
                apply_leaf_to_doc(&doc, m)?;
            }
            // 6. One commit for the whole group.
            doc.commit();

            // 7. Export exactly the new ops as one incremental update.
            let chunk_payload = doc.export_updates_since(&before)?;

            // 8. Append one immutable chunk.
            let chunk_id = next_chunk_id(tx, &doc_id)?;
            put_chunk_tx(tx, &doc_id, &chunk_id, CHUNK_FORMAT, &chunk_payload)?;

            // 9. Append one oplog row identifying the logical mutation + chunk.
            let op_id = format!("{doc_id}#{chunk_id}");
            let op_payload = serde_json::to_vec(&serde_json::json!({
                "doc_id": doc_id,
                "chunk_id": chunk_id,
                "collection": collection,
                "kind": kind,
                "record_ids": touched.iter().map(|(_, id)| id).collect::<Vec<_>>(),
            }))
            .map_err(|e| map_json("oplog payload encode", e))?;
            append_op_tx(tx, &op_id, "local", "local", chunk_id_lamport(&chunk_id), kind, &op_payload)?;

            // 10. Materialize each touched record into the projection from the
            //     post-mutation CRDT state (FTS-synced so indexes stay correct).
            for (_, id) in &touched {
                materialize_record_into_projection(tx, &doc, &collection, id, indexes)?;
            }
            Ok(leaf_count)
        })
    }
}
