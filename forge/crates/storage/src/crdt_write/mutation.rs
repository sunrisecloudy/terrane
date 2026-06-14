//! The DL-4 CRDT-backed record mutation path: a mutation becomes a Loro op on the
//! collection's [`RecordsDoc`], the incremental update is appended as one immutable
//! chunk + one oplog row, and the affected records are materialized into the
//! `records` projection — ALL inside ONE `Store::transact` closure (DL-4 atomicity).

use super::chunk_storage::{next_chunk_id, put_chunk_tx};
use super::crdt_encoding::{envelope_from_doc, load_doc_tx, write_envelope_to_doc};
use super::oplog::{append_op_tx, chunk_id_lamport, oplog_kind, OplogPayload};
use super::rebuild::materialize_record_into_projection;
use crate::index::IndexManager;
use crate::{Mutation, Store};
use forge_crdt::RecordsDoc;
use forge_domain::{CollectionId, CoreError, RecordEnvelope, RecordId, Result};
use forge_schema::{migrate_record, MigrationDescriptor};

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

/// The logical timestamp (`logical_at`) to carry on the oplog row as `mutation_at`
/// for a DELETE (DL-20 review 169). A delete tombstones the record, so no surviving
/// envelope records its WHEN — the change feed + monotone restore clock recover it
/// from this row instead. Returns the LATEST (max) delete `logical_at` among the
/// leaves (so a group ending in a delete still surfaces a non-`None` WHEN), or `None`
/// when the write touched no delete — leaving an insert/update/patch row's bytes
/// unchanged (whose WHEN is read off the surviving envelope). The timestamp is
/// clamped to a non-negative value, mirroring how `apply_leaf_to_doc` clamps the
/// envelope's logical clock.
fn delete_mutation_at(leaves: &[&Mutation]) -> Option<i64> {
    leaves
        .iter()
        .filter_map(|m| match m {
            Mutation::Delete { logical_at, .. } => Some(logical_at.unwrap_or(0).max(0)),
            _ => None,
        })
        .max()
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

/// Partition flattened leaves into per-collection buckets, preserving the order in
/// which each collection is first touched (so the write order, the per-doc chunk,
/// and the resulting oplog rows are deterministic across a multi-collection group).
/// A `RecordsDoc` is per collection (DL-2), so each bucket drives its own doc — but
/// every bucket is written inside ONE [`Store::transact`], giving a single atomic
/// boundary for the whole `transact` group (DL-4 / query-dsl.md §transact).
fn partition_by_collection<'a>(leaves: &[&'a Mutation]) -> Result<Vec<(String, Vec<&'a Mutation>)>> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<&'a Mutation>> =
        std::collections::HashMap::new();
    for m in leaves {
        let c = leaf_collection(m)?;
        if !buckets.contains_key(&c) {
            order.push(c.clone());
        }
        buckets.entry(c).or_default().push(m);
    }
    Ok(order
        .into_iter()
        .map(|c| {
            let leaves = buckets.remove(&c).expect("bucket for first-touched collection");
            (c, leaves)
        })
        .collect())
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
    /// SCOPE (M0a): a `transact` must target a **single collection**. A group that
    /// spans MORE THAN ONE collection is REJECTED with a [`CoreError::QueryError`]
    /// ("multi-collection transact is not supported") at the write boundary — see
    /// [`write_group_crdt`](Store::write_group_crdt). Such a group is locally atomic,
    /// but it persists one chunk/oplog row PER collection with NO transaction-group id,
    /// and `forge-sync` authorizes/applies each chunk INDEPENDENTLY (SS-7,
    /// `sync/src/lib.rs`), so a peer denied one collection would import a TORN half of
    /// the transaction (reviews 131/132). The full DL-17 multi-collection-atomic-SYNC
    /// (transaction-group metadata + all-or-nothing apply across the SS-7 boundary) is a
    /// separate FUTURE feature; until then a transact is confined to one collection so
    /// the locally-atomic write is also sync-safe.
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
    /// A `RecordsDoc` is per collection (DL-2). A write that spans MORE THAN ONE
    /// collection is REJECTED at this boundary (DL-17 multi-collection transact is
    /// UNSUPPORTED in M0a; see [`Store::transact_mutations_crdt`]): a cross-collection
    /// group would persist one chunk/oplog row PER collection with NO transaction-group
    /// id, and `forge-sync` authorizes/applies each chunk INDEPENDENTLY (SS-7,
    /// `sync/src/lib.rs`), so a peer denied one collection would import a TORN half of
    /// the transaction (reviews 131/132). Until the SS-7 boundary carries
    /// transaction-group metadata and applies it all-or-nothing (DL-17
    /// multi-collection-atomic-SYNC, a separate future feature), a transact is confined
    /// to ONE collection so the locally-atomic write is also sync-safe.
    ///
    /// A single-collection group writes one chunk + one oplog row inside ONE
    /// `Store::transact` (byte-stable with the prior path): load-or-rebuild the doc
    /// from chunks, capture `before_version`, apply the leaves to the doc, commit the
    /// doc, export the incremental update since `before_version`, append it as one
    /// immutable chunk, append one oplog row, then re-materialize each touched record
    /// into the `records` projection (FTS-synced). Returns the leaf count.
    fn write_group_crdt(
        &mut self,
        top: &[Mutation],
        kind: &str,
        indexes: &IndexManager,
    ) -> Result<usize> {
        // Flatten to leaves, then partition into per-collection buckets (first-touch
        // order). A multi-collection `transact` is rejected below; a single-collection
        // write is the one-bucket case written under one transaction.
        let mut leaves: Vec<&Mutation> = Vec::new();
        for m in top {
            flatten_leaves(m, &mut leaves);
        }
        if leaves.is_empty() {
            return Ok(0);
        }
        let buckets = partition_by_collection(&leaves)?;
        // SCOPE: a transact may span only ONE collection (DL-17 multi-collection
        // transact is unsupported in M0a — it is NOT sync-safe across the SS-7 boundary;
        // see this function's doc comment and reviews 131/132). Reject a cross-collection
        // group BEFORE any CRDT/SQLite work, so neither collection is written.
        if buckets.len() > 1 {
            let collections: Vec<&str> = buckets.iter().map(|(c, _)| c.as_str()).collect();
            return Err(CoreError::QueryError(format!(
                "multi-collection transact is not supported (DL-17): a transact must target a \
                 single collection, but this group spans {collections:?}"
            )));
        }
        let leaf_count = leaves.len();
        let peer_id = self.crdt_peer_id();

        self.transact(|tx| {
            for (collection, bucket) in &buckets {
                write_collection_bucket_tx(tx, collection, bucket, kind, peer_id, indexes)?;
            }
            Ok(leaf_count)
        })
    }
}

/// Apply ONE collection's leaves to its `RecordsDoc` and persist the chunk + oplog
/// row + projection rows, inside an already-open transaction `tx`. Called once per
/// collection bucket by [`Store::write_group_crdt`]; sharing `tx` across buckets is
/// what makes a multi-collection group atomic (a failure here rolls the whole
/// transaction back). `kind` is the oplog kind for the whole logical write.
fn write_collection_bucket_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
    leaves: &[&Mutation],
    kind: &str,
    peer_id: u64,
    indexes: &IndexManager,
) -> Result<()> {
    let doc_id = collection_doc_id(collection);
    let touched = touched_ids(leaves);

    // 1-4. Load/reconstruct the doc and capture the pre-mutation version.
    let doc = load_doc_tx(tx, &doc_id, peer_id)?;
    let before = doc.version();

    // 5. Apply every leaf to the doc.
    for m in leaves {
        apply_leaf_to_doc(&doc, m)?;
    }
    // 6. One commit for this collection's slice of the group.
    doc.commit();

    // 7. Export exactly the new ops as one incremental update.
    let chunk_payload = doc.export_updates_since(&before)?;

    // 8. Append one immutable chunk (per collection doc).
    let chunk_id = next_chunk_id(tx, &doc_id)?;
    put_chunk_tx(tx, &doc_id, &chunk_id, CHUNK_FORMAT, &chunk_payload)?;

    // 9. Append one oplog row identifying the logical mutation + chunk. The payload
    //    schema is owned by `OplogPayload` (shared with the remote import path) so
    //    the two cannot skew. The op_id is `(doc_id)#(chunk_id)`, unique per doc, so
    //    distinct collections in one group never collide. A delete carries its
    //    `mutation_at` (DL-20 review 169) so the tombstoned version's WHEN survives in
    //    the change feed even though no envelope does; non-delete writes pass `None`
    //    (their WHEN is read off the surviving envelope) and the row stays byte-stable.
    let op_id = format!("{doc_id}#{chunk_id}");
    let op_payload = OplogPayload::local(
        &doc_id,
        &chunk_id,
        collection,
        kind,
        touched.iter().map(|(_, id)| id.to_string()).collect(),
        delete_mutation_at(leaves),
    )
    .encode("oplog payload encode")?;
    append_op_tx(tx, &op_id, "local", "local", chunk_id_lamport(&chunk_id), kind, &op_payload)?;

    // 10. Materialize each touched record into the projection from the post-mutation
    //     CRDT state (FTS-synced so indexes stay correct).
    for (_, id) in &touched {
        materialize_record_into_projection(tx, &doc, collection, id, indexes)?;
    }
    Ok(())
}

/// Apply a DL-13 [`MigrationDescriptor`] to **every** record of its collection by
/// rewriting the CRDT **source of truth** (not just the derived `records`
/// projection), inside an already-open transaction `tx`. Returns the ids of the
/// records transformed (in deterministic doc order), which the caller records in
/// the `schema.migration` oplog op.
///
/// This is the durability seam for review 138 P1: the migration must live in the
/// same `crdt_chunks` stream that DL-6 [`rebuild_projection`](Store::rebuild_projection)
/// replays, or a rebuild would restore the PRE-migration values while
/// `schema_version` stayed advanced. So we:
/// 1. Load the collection doc from chunks (source of truth) and capture `before`.
/// 2. For each record id in the doc, read its envelope, apply [`migrate_record`]
///    (the pure transform — a non-coercible value propagates its typed error and
///    rolls the whole transaction back), and write the migrated envelope back into
///    the doc.
/// 3. Commit once and export exactly the new ops as ONE incremental chunk, appended
///    immutably (so the migration rides Loro history and survives rebuild/sync).
/// 4. Materialize each transformed record into the projection (FTS-synced).
///
/// A record whose value the transform leaves byte-identical still re-exports as a
/// no-op delta; only genuinely changed records add ops, keeping the chunk minimal
/// and rebuild byte-equal.
///
/// `registry_collection` is the affected collection's EVOLVED registry entry (a
/// serialized `forge_schema::CollectionDef`), carried into the per-chunk migration
/// oplog row so an authorized receiver evolves its `SchemaRegistry` in lockstep with
/// the migrated records + `schema_version` (review 143). `None` for a migration the
/// caller drives without a registry change (e.g. the storage unit tests).
pub(crate) fn migrate_collection_records_crdt_tx(
    tx: &rusqlite::Transaction<'_>,
    descriptor: &MigrationDescriptor,
    peer_id: u64,
    indexes: &IndexManager,
    registry_collection: Option<&serde_json::Value>,
) -> Result<Vec<String>> {
    let collection = descriptor.collection.as_str();
    let doc_id = collection_doc_id(collection);

    let doc = load_doc_tx(tx, &doc_id, peer_id)?;
    let before = doc.version();

    // Deterministic order: list_record_ids is sorted, so the chunk/oplog the
    // migration produces is byte-stable across runs (the DL-13 determinism
    // contract carried through to the source of truth).
    let mut record_ids = doc.list_record_ids();
    record_ids.sort();

    for id in &record_ids {
        let Some(prior) = envelope_from_doc(&doc, id)? else {
            continue; // CRDT-deleted between listing and read — nothing to migrate.
        };
        let migrated = migrate_record(&prior, descriptor)?;
        write_envelope_to_doc(&doc, &migrated)?;
    }
    doc.commit();

    // Export and persist the migration as ONE immutable chunk on the collection doc.
    let chunk_payload = doc.export_updates_since(&before)?;
    let chunk_id = next_chunk_id(tx, &doc_id)?;
    put_chunk_tx(tx, &doc_id, &chunk_id, CHUNK_FORMAT, &chunk_payload)?;

    // Append the PER-CHUNK oplog row keyed `(doc_id)#(chunk_id)`, the SAME scheme an
    // ordinary mutation chunk uses (see `write_collection_bucket_tx`). This is what
    // makes the migration chunk DISCOVERABLE on the sync path (review 139): the sync
    // seam joins chunks → metadata by `op_id = "{doc_id}#{chunk_id}"`
    // (`missing_chunks_for_doc`), so without this row a migration chunk fell back to a
    // generic write with empty `record_ids` and the RBAC gate dropped it at peer sync.
    // The row carries the migrated `record_ids` (so the chunk authorizes as a record
    // write against concrete ids) AND the `from`/`to` schema versions (so a receiver
    // advances its `schema_version` to `to` on import). The separate `schema.migration`
    // AUDIT row (keyed `migration#<from>-<to>#<collection>`) is still appended by the
    // caller; only THIS per-chunk row participates in the sync join.
    let op_id = format!("{doc_id}#{chunk_id}");
    let op_payload = OplogPayload::migration(
        &doc_id,
        &chunk_id,
        collection,
        crate::migration::MIGRATION_OP_KIND,
        record_ids.clone(),
        descriptor.from_schema_version,
        descriptor.to_schema_version,
        // The affected collection's evolved registry entry (review 143), so an
        // authorized receiver evolves its SchemaRegistry in lockstep with the migrated
        // records + version. `None` for a migration driven without a registry change.
        registry_collection.cloned(),
    )
    .encode("migration chunk oplog payload encode")?;
    append_op_tx(
        tx,
        &op_id,
        "local",
        "local",
        chunk_id_lamport(&chunk_id),
        crate::migration::MIGRATION_OP_KIND,
        &op_payload,
    )?;

    // Materialize every transformed record into the projection from the migrated
    // CRDT state (FTS-synced), so the maintained projection matches a from-scratch
    // rebuild byte-for-byte.
    for id in &record_ids {
        materialize_record_into_projection(tx, &doc, collection, id, indexes)?;
    }
    Ok(record_ids)
}
