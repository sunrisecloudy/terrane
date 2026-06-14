//! forge-storage CRDT-backed record write path (DL-4) + projection rebuild (DL-6).
//!
//! Normative spec: `prd-merged/02-data-layer-prd.md` DL-1..6/DL-17/DL-21 and
//! `forge/spec/crdt-write-path.md`. This module makes the **CRDT docs the source
//! of truth** and the `records` table a *derived, rebuildable projection*:
//!
//! - Each collection is one [`RecordsDoc`](forge_crdt::RecordsDoc) addressed by
//!   `doc_id = "collection/<name>"` (see [`collection_doc_id`]). The Loro map keys
//!   are record ids and each value is the record's **full serialized
//!   [`RecordEnvelope`]** — so materializing the projection is "read the doc, write
//!   the row", which is exactly what rebuild does, giving zero diff by construction.
//! - A record mutation becomes a Loro op on that doc; the incremental update is
//!   appended as an immutable `crdt_chunks` row, an `oplog` row is appended, and
//!   the `records` projection is materialized — **all in one SQLite transaction**
//!   (DL-4). A failure rolls back the chunk, the op, and the projection together.
//! - [`Store::rebuild_projection`] (DL-6) drops the entire `records` projection and
//!   reconstructs it purely from `crdt_chunks` via the Loro `from_updates` rebuild
//!   primitive, then rebuilds active indexes. It must equal the maintained
//!   projection with zero diff.
//!
//! The CRDT path is **added alongside** the existing projection-only write methods
//! (`put_record`, `apply_mutation`, …) which are preserved as the rebuild/raw seam;
//! `apply_mutation_crdt` / `transact_mutations_crdt` are the real DL-4 mutation
//! surface that the spine routes through.

use crate::index::IndexManager;
use crate::{map_json, map_sql, now_ms, put_record_tx, Mutation, Store};
use forge_crdt::RecordsDoc;
use forge_domain::{CollectionId, CoreError, RecordEnvelope, RecordId, Result};
use rusqlite::params;

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

/// The stable oplog `kind` string for a logical mutation, e.g. `record.insert`.
/// Matches the fixtures' `expect_oplog_kinds`.
fn oplog_kind(m: &Mutation) -> &'static str {
    match m {
        Mutation::Insert { .. } => "record.insert",
        Mutation::Update { .. } => "record.update",
        Mutation::Patch { .. } => "record.patch",
        Mutation::Delete { .. } => "record.delete",
        Mutation::Transact { .. } => "record.transact",
    }
}

/// Mint the next immutable chunk id for `doc_id`. Ids are `chunk-NNNN`, zero-padded
/// so lexical order matches insertion order (the `get_chunks` ordering tiebreak),
/// and the sequence is the count of existing chunks + 1 so a re-run never collides
/// with a prior chunk (append-only discipline, review 003). Computed inside the
/// open transaction so it sees only committed chunks.
fn next_chunk_id(tx: &rusqlite::Transaction<'_>, doc_id: &str) -> Result<String> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM crdt_chunks WHERE doc_id = ?1",
            params![doc_id],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    Ok(format!("chunk-{:04}", count + 1))
}

/// Append one immutable CRDT chunk inside an open transaction. Mirrors
/// [`Store::put_chunk`]'s append-only contract (review 003) but tx-scoped: an
/// identical re-write is an idempotent no-op, a conflicting payload under an
/// existing `(doc_id, chunk_id)` is a `StorageError`.
fn put_chunk_tx(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    chunk_id: &str,
    format: &str,
    payload: &[u8],
) -> Result<()> {
    let existing: Option<(String, Vec<u8>)> = tx
        .query_row(
            "SELECT format, payload FROM crdt_chunks WHERE doc_id = ?1 AND chunk_id = ?2",
            params![doc_id, chunk_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional_row()?;
    if let Some((ef, ep)) = existing {
        if ef == format && ep == payload {
            return Ok(());
        }
        return Err(CoreError::StorageError(format!(
            "crdt chunk ({doc_id}, {chunk_id}) is append-only and already exists \
             with different content; refusing to rewrite history"
        )));
    }
    tx.execute(
        "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![doc_id, chunk_id, format, payload, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Append one oplog row inside an open transaction (the DL-4 write metadata that
/// identifies the logical mutation, its doc id, and the chunk it produced). The
/// `op_id` is `(doc_id)#(chunk_id)`, unique because chunk ids are unique per doc.
#[allow(clippy::too_many_arguments)]
fn append_op_tx(
    tx: &rusqlite::Transaction<'_>,
    op_id: &str,
    actor_id: &str,
    workspace_id: &str,
    lamport: u64,
    kind: &str,
    payload: &[u8],
) -> Result<()> {
    tx.execute(
        "INSERT INTO oplog
             (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![op_id, actor_id, workspace_id, lamport as i64, kind, payload, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// A tiny extension so the tx-scoped chunk read can use `.optional()` semantics
/// without pulling the whole `OptionalExtension` import into scope confusingly.
trait OptionalRow<T> {
    fn optional_row(self) -> Result<Option<T>>;
}
impl<T> OptionalRow<T> for std::result::Result<T, rusqlite::Error> {
    fn optional_row(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(map_sql(e)),
        }
    }
}

/// Load (or reconstruct) a collection's `RecordsDoc` from its persisted chunks,
/// reading inside the open transaction so it sees only committed history (DL-4
/// step 3 / DL-6 rebuild primitive). An empty chunk set yields a fresh document.
fn load_doc_tx(tx: &rusqlite::Transaction<'_>, doc_id: &str, peer_id: u64) -> Result<RecordsDoc> {
    let mut stmt = tx
        .prepare("SELECT payload FROM crdt_chunks WHERE doc_id = ?1 ORDER BY created_at, chunk_id")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(params![doc_id], |row| row.get::<_, Vec<u8>>(0))
        .map_err(map_sql)?;
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for r in rows {
        payloads.push(r.map_err(map_sql)?);
    }
    let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_slice()).collect();
    // The peer id governs the identity of FUTURE ops written under this loaded
    // doc — distinct per store so two synced peers' concurrent same-scalar edits
    // converge to one Loro LWW winner (SS-1/SS-2). Imported history is unaffected.
    RecordsDoc::from_updates(peer_id, &refs)
}

/// Read the materialized [`RecordEnvelope`] for `id` out of a `RecordsDoc`, or
/// `None` if the record is absent (never written, or CRDT-deleted). The doc stores
/// the full envelope JSON per record (see module docs), so this is a direct decode.
fn envelope_from_doc(doc: &RecordsDoc, id: &str) -> Result<Option<RecordEnvelope>> {
    match doc.get_record(id) {
        Some(value) => {
            let env: RecordEnvelope = serde_json::from_value(value)
                .map_err(|e| map_json("crdt envelope decode", e))?;
            Ok(Some(env))
        }
        None => Ok(None),
    }
}

/// Write a record's full envelope into the `RecordsDoc` as the record's value. We
/// `replace_record_fields` with the *entire* envelope JSON because the caller has
/// already read-modify-merged it (so a full replace is the post-state); the doc's
/// value then equals the envelope byte-for-byte, which is what makes DL-6 rebuild
/// reproduce the maintained projection exactly.
fn write_envelope_to_doc(doc: &RecordsDoc, env: &RecordEnvelope) -> Result<()> {
    let value =
        serde_json::to_value(env).map_err(|e| map_json("crdt envelope encode", e))?;
    // Write the envelope with its nested-object values (`fields`, `field_ids`)
    // mapped onto nested Loro map *containers* — one register per leaf field — so
    // two peers concurrently editing DIFFERENT fields of the same record both
    // survive the merge (SS-1/SS-2, DL-3/DL-9). A flat whole-`fields` register
    // would collide and lose one writer's edit. The materialized value still
    // equals the envelope byte-for-byte, so DL-6 rebuild is unaffected.
    doc.write_record_envelope(env.entity_id.as_str(), &value)
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

    /// DL-6 projection rebuild: drop the **entire** `records` projection and
    /// reconstruct it purely from the persisted `crdt_chunks` (the CRDT docs are
    /// the source of truth), then rebuild active indexes. Must equal the
    /// incrementally maintained projection with zero diff.
    ///
    /// For every distinct `doc_id` present in `crdt_chunks` that names a collection
    /// (`collection/<name>`), the chunks are folded back into a fresh `RecordsDoc`
    /// via the Loro `from_updates` rebuild primitive (order/duplication independent)
    /// and each live record is re-materialized. Records CRDT-deleted in history are
    /// simply absent from the rebuilt doc, so they do not reappear (DL-21).
    pub fn rebuild_projection(&mut self, indexes: &IndexManager) -> Result<()> {
        let peer_id = self.crdt_peer_id();
        self.transact(|tx| rebuild_projection_tx(tx, peer_id, indexes))
    }

    /// Atomically apply a batch of chunks that arrived from a **remote peer**
    /// during sync, in ONE SQLite transaction (DL-4: "Remote updates follow the
    /// identical path"; review 088 #1). For the whole batch this:
    ///
    /// 1. inserts each missing chunk into `crdt_chunks` (append-only, idempotent —
    ///    an identical re-import adds no row, a conflicting payload trips the
    ///    history-rewrite guard) and, only for a newly-inserted chunk, appends one
    ///    matching `oplog` row tagged with the remote `source`;
    /// 2. rebuilds the records projection AND active physical indexes from the
    ///    now-augmented chunk history — all inside the SAME transaction.
    ///
    /// Either the entire receiving-store update commits, or it rolls back wholly:
    /// a failure in any chunk insert, the oplog append, or the projection/index
    /// rebuild leaves `crdt_chunks`, `oplog`, and `records` byte-for-byte as they
    /// were before the call. This closes the gap where per-chunk commits + a
    /// post-hoc rebuild could leave committed chunk/oplog rows under a stale
    /// projection if a later import or the rebuild failed.
    ///
    /// `indexes` must be the RECEIVING store's OWN [`IndexManager`] (review 084
    /// #1): index metadata is per-store and not part of the synced chunk payload,
    /// so rebuilding against a foreign manager would issue index DML for tables
    /// this store lacks (or skip the ones it has). Returns the number of chunks
    /// newly imported (a fully-converged re-apply returns `0` and is a pure no-op:
    /// no chunk, no oplog row, and the projection is rebuilt to the identical
    /// state).
    pub fn apply_remote_chunks(
        &mut self,
        chunks: &[RemoteChunk],
        source: &str,
        indexes: &IndexManager,
    ) -> Result<usize> {
        let peer_id = self.crdt_peer_id();
        let source = source.to_string();
        self.transact(move |tx| {
            let mut imported = 0usize;
            for chunk in chunks {
                if import_remote_chunk_tx(tx, chunk, &source)? {
                    imported += 1;
                }
            }
            // Rebuild the projection + active physical indexes from the augmented
            // chunk history INSIDE this transaction, so a rebuild failure rolls the
            // chunk/oplog inserts back with it (atomic per receiving store).
            rebuild_projection_tx(tx, peer_id, indexes)?;
            Ok(imported)
        })
    }
}

/// One chunk handed to [`Store::apply_remote_chunks`]: the receiving-store chunk id
/// (content-addressed by the sync seam) plus the immutable `(format, payload)` and
/// the ORIGINAL-author provenance the import must persist so a later relay hop can
/// recover it (`review 092 #1`).
#[derive(Debug, Clone)]
pub struct RemoteChunk {
    /// The `doc_id` the chunk belongs to (`collection/<name>`).
    pub doc_id: String,
    /// The id the chunk is stored under in the receiver (the sync seam's
    /// content-addressed exchanged id, network-safe across peers).
    pub chunk_id: String,
    /// The chunk encoding tag (`loro`), preserved verbatim.
    pub format: String,
    /// The opaque immutable Loro update bytes.
    pub payload: Vec<u8>,
    /// The chunk's ORIGINAL author, when known to differ from the importing
    /// `source`. `Some(peer:<id>)` when the chunk was itself FORWARDED (the sender
    /// only relayed it and recovered the original author from its own provenance);
    /// `None` when the importing `source` IS the author (a first-hop import of a
    /// locally-authored chunk). The remote-import oplog row records this original
    /// author as its `source`, so a peer that re-exports this chunk preserves the
    /// true author across the next hop rather than overwriting it with itself.
    pub author_actor_id: Option<String>,
    /// The record ids the chunk touched, recovered from the origin's op metadata.
    /// Preserved into the remote-import oplog row so a later hop's authorization
    /// envelope still names a concrete record (the SS-7 resource gate / `review
    /// 092 #2` envelope-metadata check), instead of failing closed on a forwarded
    /// chunk whose record identity was dropped at import.
    pub record_ids: Vec<String>,
}

/// Import ONE remote chunk inside an open transaction: append-only insert into
/// `crdt_chunks` plus, only when the chunk is newly inserted, one matching `oplog`
/// row tagged with the remote `source` (DL-4 remote parity). Returns `true` iff a
/// new chunk (and its oplog row) was written; an identical re-import is an
/// idempotent `false` no-op, and a conflicting payload under an existing chunk id
/// trips the append-only history-rewrite guard.
///
/// This is the SINGLE remote-import code path. [`Store::apply_remote_chunks`] calls
/// it per chunk, and the single-chunk [`Store::put_chunk_from_remote`] delegates to
/// `apply_remote_chunks`, so EVERY public remote import funnels through here and none
/// can emit a provenance-poor `record.remote_import` row (`review 095`) nor skip the
/// in-transaction projection/index rebuild (`review 090 #3`). The original author and
/// touched record ids ride on the [`RemoteChunk`], never on a divergent inline insert.
pub(crate) fn import_remote_chunk_tx(
    tx: &rusqlite::Transaction<'_>,
    chunk: &RemoteChunk,
    source: &str,
) -> Result<bool> {
    let RemoteChunk { doc_id, chunk_id, format, payload, author_actor_id, record_ids } = chunk;
    let existing: Option<(String, Vec<u8>)> = tx
        .query_row(
            "SELECT format, payload FROM crdt_chunks WHERE doc_id = ?1 AND chunk_id = ?2",
            params![doc_id, chunk_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional_row()?;
    if let Some((ef, ep)) = existing {
        if &ef == format && &ep == payload {
            // Already imported — DO NOT append a duplicate oplog row.
            return Ok(false);
        }
        return Err(CoreError::StorageError(format!(
            "crdt chunk ({doc_id}, {chunk_id}) is append-only and already exists \
             with different content; refusing to rewrite history"
        )));
    }
    tx.execute(
        "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![doc_id, chunk_id, format, payload, now_ms()],
    )
    .map_err(map_sql)?;

    // One oplog row in the SAME transaction, tagged remote (DL-4 parity). op_id
    // mirrors the local path: `(doc_id)#(chunk_id)`, unique because chunk ids are
    // unique per doc. The lamport is derived from a local `chunk-NNNN` id, else 0.
    //
    // The recorded `source` is the chunk's ORIGINAL author: `author_actor_id` when
    // the chunk was itself forwarded (the importing `source` is only a relay that
    // recovered the real author from its provenance), else the importing `source`
    // (a first-hop import of a locally-authored chunk, where the source IS the
    // author). Persisting the original author — together with the touched
    // `record_ids` — means a peer that later re-exports this chunk preserves the true
    // author and record identity across the next relay hop, so the receiver still
    // gates the ORIGINAL actor and names a concrete record (`review 092 #1/#2`).
    let op_id = format!("{doc_id}#{chunk_id}");
    let lamport = chunk_id_lamport(chunk_id);
    let original_author = author_actor_id.as_deref().unwrap_or(source);
    let op_payload = serde_json::to_vec(&serde_json::json!({
        "doc_id": doc_id,
        "chunk_id": chunk_id,
        "kind": "record.remote_import",
        "source": original_author,
        "record_ids": record_ids,
    }))
    .map_err(|e| map_json("remote oplog payload encode", e))?;
    append_op_tx(
        tx,
        &op_id,
        original_author,
        "remote",
        lamport,
        "record.remote_import",
        &op_payload,
    )?;
    Ok(true)
}

/// DL-6 projection rebuild inside a caller-provided transaction: drop the whole
/// `records` projection, re-materialize it from `crdt_chunks` (the CRDT source of
/// truth), and rebuild active physical indexes — all on `tx` so the rebuild commits
/// or rolls back with whatever else the caller did in the same transaction. The
/// shared engine behind [`Store::rebuild_projection`] and
/// [`Store::apply_remote_chunks`].
fn rebuild_projection_tx(
    tx: &rusqlite::Transaction<'_>,
    peer_id: u64,
    indexes: &IndexManager,
) -> Result<()> {
    // Drop the whole projection — it is derived, so this is safe.
    tx.execute("DELETE FROM records", []).map_err(map_sql)?;

    // Every collection doc with persisted chunks.
    let doc_ids: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT DISTINCT doc_id FROM crdt_chunks ORDER BY doc_id")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        out
    };

    for doc_id in &doc_ids {
        let Some(collection) = collection_of_doc(doc_id) else {
            continue; // not a records collection doc (e.g. an applet src doc)
        };
        let doc = load_doc_tx(tx, doc_id, peer_id)?;
        for id in doc.list_record_ids() {
            materialize_record_into_projection(tx, &doc, collection, &id, indexes)?;
        }
    }

    // Rebuild active physical indexes from the freshly materialized records, IN
    // the same transaction (a Transaction derefs to &Connection), so an index
    // failure rolls the projection rebuild back with it.
    indexes.rebuild_active(tx)
}

/// Re-materialize ONE record into the `records` projection from the post-mutation
/// CRDT doc, inside the open transaction, keeping active FTS in sync (DL-5). If the
/// record is absent in the doc (CRDT-deleted), the projection row is removed so the
/// maintained projection and a from-scratch rebuild agree exactly.
fn materialize_record_into_projection(
    tx: &rusqlite::Transaction<'_>,
    doc: &RecordsDoc,
    collection: &str,
    id: &str,
    indexes: &IndexManager,
) -> Result<()> {
    match envelope_from_doc(doc, id)? {
        Some(env) => {
            let data = serde_json::to_string(&env)
                .map_err(|e| map_json("materialize record", e))?;
            put_record_tx(tx, &env)?;
            indexes.sync_fts_for_record(tx, collection, id, &data)
        }
        None => {
            // CRDT-deleted: drop the projection row and any FTS shadow row.
            tx.execute(
                "DELETE FROM records WHERE collection = ?1 AND id = ?2",
                params![collection, id],
            )
            .map_err(map_sql)?;
            indexes.delete_fts_for_record(tx, collection, id)
        }
    }
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

/// The collection name encoded in a `collection/<name>` doc id, or `None` if the
/// doc id is not a records-collection doc.
///
/// Public so the sync seam (and the SS-7 authorization gate above it) can derive
/// the target collection of an incoming chunk straight from its `doc_id` without
/// re-parsing the prefix, mirroring [`collection_doc_id`].
pub fn collection_of_doc(doc_id: &str) -> Option<&str> {
    doc_id.strip_prefix("collection/")
}

/// Derive a monotone-ish lamport for the oplog from the chunk sequence number, so
/// the oplog's `(lamport, op_id)` total order matches write order without a
/// separate clock. `chunk-0007` → lamport 7. Malformed ids fall back to 0.
fn chunk_id_lamport(chunk_id: &str) -> u64 {
    chunk_id
        .strip_prefix("chunk-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateIndexKind, Mutation, Query, QueryResult};
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

    /// The visible projection as an ordered list of `{id, fields}` (deleted rows
    /// hidden), matching the fixtures' `expect_records` shape.
    fn visible_records(s: &Store, collection: &str) -> Vec<serde_json::Value> {
        let q = Query::from(collection);
        match s.query(&q).unwrap() {
            QueryResult::Rows(rows) => rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.envelope.entity_id.as_str(),
                        "fields": serde_json::to_value(&r.envelope.fields).unwrap(),
                    })
                })
                .collect(),
            other => panic!("expected rows, got {other:?}"),
        }
    }

    /// A full snapshot of the projection (every row's canonical `data` keyed by
    /// `collection/id`) for zero-diff comparison across a rebuild.
    fn projection_snapshot(s: &Store) -> std::collections::BTreeMap<String, serde_json::Value> {
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
        let mut out = std::collections::BTreeMap::new();
        for r in rows {
            let (c, id, data) = r.unwrap();
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            out.insert(format!("{c}/{id}"), value);
        }
        out
    }

    // --- DL-4: the single-mutation write chain ---------------------------

    #[test]
    fn insert_writes_chunk_oplog_and_projection_in_one_pass() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();

        // Projection materialized.
        let env = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(env.fields["title"], json!("Ship"));
        // Exactly one chunk on the collection doc.
        let chunks = s.get_chunks(&collection_doc_id("tasks")).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].format, CHUNK_FORMAT);
        assert_eq!(chunks[0].chunk_id, "chunk-0001");
        // Exactly one oplog row, kind record.insert.
        let ops = s.list_ops().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, "record.insert");
    }

    #[test]
    fn insert_materializes_stable_field_ids() {
        // The CRDT insert path must preserve the review 045/046/049 field_id
        // materialization so indexes keyed on the stable id still see the record.
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let env = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(env.field_ids.get("f_title"), Some(&json!("Ship")));
    }

    #[test]
    fn patch_preserves_omitted_and_unknown_fields() {
        let mut s = store();
        let idx = IndexManager::new();
        // Seed with a display field and an unknown/forward-compat field via the
        // CRDT path (insert carries display fields; inject unknown by a raw write
        // is not available here, so assert display-field preservation + DL-9).
        s.apply_mutation_crdt(
            &insert("tasks", "t1", json!({"title": "A", "status": "draft", "prio": 1}), 1),
            &idx,
        )
        .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"status": "ready"}), 2), &idx)
            .unwrap();
        let env = s.get_record("tasks", "t1").unwrap().unwrap();
        assert_eq!(env.fields["title"], json!("A"), "omitted display field preserved");
        assert_eq!(env.fields["status"], json!("ready"));
        assert_eq!(env.fields["prio"], json!(1));
    }

    // --- DL-4 delete + the one-transaction atomicity ----------------------

    #[test]
    fn delete_removes_record_from_projection_and_keeps_history() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Temp"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx).unwrap();
        // Gone from the projection.
        assert!(s.get_record("tasks", "t1").unwrap().is_none());
        assert!(visible_records(&s, "tasks").is_empty());
        // Two chunks (insert + delete) survive in append-only history.
        assert_eq!(s.get_chunks(&collection_doc_id("tasks")).unwrap().len(), 2);
        assert_eq!(s.list_ops().unwrap().last().unwrap().kind, "record.delete");
    }

    #[test]
    fn failed_write_rolls_back_both_chunk_and_projection() {
        // A patch of a missing record fails: NEITHER a chunk NOR a projection row
        // may be left behind (DL-4 one-transaction atomicity).
        let mut s = store();
        let idx = IndexManager::new();
        let err = s
            .apply_mutation_crdt(&patch("tasks", "ghost", json!({"x": 1}), 1), &idx)
            .unwrap_err();
        assert_eq!(err.code(), "QueryError");
        // No chunk, no oplog row, no projection row.
        assert!(s.get_chunks(&collection_doc_id("tasks")).unwrap().is_empty());
        assert!(s.list_ops().unwrap().is_empty());
        assert!(s.get_record("tasks", "ghost").unwrap().is_none());
    }

    #[test]
    fn failed_group_rolls_back_the_whole_transaction() {
        let mut s = store();
        let idx = IndexManager::new();
        // First a good write so there IS prior history to preserve.
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"v": 1}), 1), &idx)
            .unwrap();
        let chunks_before = s.get_chunks(&collection_doc_id("tasks")).unwrap().len();

        // A group whose second leaf patches a missing record -> whole group fails.
        let items = vec![
            insert("tasks", "t2", json!({"v": 2}), 2),
            patch("tasks", "missing", json!({"v": 3}), 3),
        ];
        let err = s.transact_mutations_crdt(&items, &idx).unwrap_err();
        assert_eq!(err.code(), "QueryError");
        // t2 was not materialized and no new chunk/op landed.
        assert!(s.get_record("tasks", "t2").unwrap().is_none());
        assert_eq!(
            s.get_chunks(&collection_doc_id("tasks")).unwrap().len(),
            chunks_before,
            "a failed group must not append a chunk"
        );
        // The prior good record is untouched.
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap().fields["v"], json!(1));
    }

    // --- DL-4 transact group: one commit -> one chunk --------------------

    #[test]
    fn transact_group_collapses_to_one_chunk_and_one_oplog_row() {
        let mut s = store();
        let idx = IndexManager::new();
        let items = vec![
            insert("tasks", "t1", json!({"title": "A", "done": false}), 1),
            insert("tasks", "t2", json!({"title": "B", "done": false}), 2),
            patch("tasks", "t1", json!({"done": true}), 3),
        ];
        let n = s.transact_mutations_crdt(&items, &idx).unwrap();
        assert_eq!(n, 3);
        // One chunk, one oplog row of kind record.transact.
        assert_eq!(s.get_chunks(&collection_doc_id("tasks")).unwrap().len(), 1);
        let ops = s.list_ops().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, "record.transact");
        // Post-state projection.
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap().fields["done"], json!(true));
        assert_eq!(s.get_record("tasks", "t2").unwrap().unwrap().fields["title"], json!("B"));
    }

    // --- apply_remote_chunks: atomic per-store remote apply (review 088 #1) --

    /// Count `crdt_chunks` rows across every doc (a substrate-level total).
    fn total_chunk_rows(s: &Store) -> i64 {
        s.connection()
            .query_row("SELECT COUNT(*) FROM crdt_chunks", [], |r| r.get(0))
            .unwrap()
    }

    /// Borrow one valid `loro` chunk verbatim out of a store's persisted history,
    /// to re-feed it as a `RemoteChunk` to a different store.
    fn one_chunk(s: &Store, doc_id: &str) -> RemoteChunk {
        let c = s.get_chunks(doc_id).unwrap().into_iter().next().unwrap();
        RemoteChunk {
            doc_id: doc_id.to_string(),
            chunk_id: c.chunk_id,
            format: c.format,
            payload: c.payload,
            author_actor_id: None,
            record_ids: Vec::new(),
        }
    }

    #[test]
    fn apply_remote_chunks_imports_and_rebuilds_atomically() {
        // A clean remote apply: the staged chunk lands in crdt_chunks + oplog and
        // the projection materializes — all committed together. A second apply of
        // the same chunk is a pure idempotent no-op (no new chunk/oplog row).
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let chunk = one_chunk(&src, &doc_id);

        let mut dst = store();
        let imported = dst
            .apply_remote_chunks(std::slice::from_ref(&chunk), "peer:7", &idx)
            .unwrap();
        assert_eq!(imported, 1, "one new chunk imported");
        // Chunk + a remote-tagged oplog row landed, and the projection rebuilt.
        assert_eq!(total_chunk_rows(&dst), 1);
        let ops = dst.list_ops().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, "record.remote_import");
        assert_eq!(ops[0].actor_id, "peer:7");
        assert_eq!(ops[0].workspace_id, "remote");
        assert_eq!(dst.get_record("tasks", "t1").unwrap().unwrap().fields["title"], json!("Ship"));

        // Idempotent re-apply: no new chunk, no new oplog row, same projection.
        let again = dst
            .apply_remote_chunks(std::slice::from_ref(&chunk), "peer:7", &idx)
            .unwrap();
        assert_eq!(again, 0, "re-applying a present chunk imports nothing");
        assert_eq!(total_chunk_rows(&dst), 1, "no duplicate chunk row");
        assert_eq!(dst.list_ops().unwrap().len(), 1, "no duplicate oplog row");
    }

    #[test]
    fn forwarded_chunk_import_preserves_original_author_and_record_ids() {
        // review 092 #1/#2: when a chunk is FORWARDED (the importing `source` is only a
        // relay), the remote-import oplog row must record the chunk's ORIGINAL author —
        // carried in `RemoteChunk::author_actor_id` — and the touched `record_ids`, NOT
        // the importing relay's source. This is what lets a later hop's authorization
        // gate against the original actor and still name a concrete record.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let mut chunk = one_chunk(&src, &doc_id);
        // Mark the chunk as forwarded: its original author is peer:C, and it touched t1.
        chunk.author_actor_id = Some("peer:C".to_string());
        chunk.record_ids = vec!["t1".to_string()];

        let mut relay = store();
        // The importing source is the RELAY (peer:A), distinct from the original author.
        relay
            .apply_remote_chunks(std::slice::from_ref(&chunk), "peer:A", &idx)
            .unwrap();

        let ops = relay.list_ops().unwrap();
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.kind, "record.remote_import");
        // The oplog attributes the ORIGINAL author, not the relay (peer:A).
        assert_eq!(op.actor_id, "peer:C", "the import preserves the ORIGINAL author");
        let payload: serde_json::Value = serde_json::from_slice(&op.payload).unwrap();
        assert_eq!(payload["source"], json!("peer:C"), "payload source is the original author");
        assert_eq!(payload["record_ids"], json!(["t1"]), "the touched record id is preserved");
    }

    #[test]
    fn legacy_put_chunk_from_remote_preserves_forwarded_provenance() {
        // review 095: the still-public single-chunk import escape hatch must funnel
        // through the SAME provenance-preserving engine as `apply_remote_chunks`, so
        // it CANNOT emit a provenance-poor `record.remote_import`. When the caller
        // supplies the chunk's ORIGINAL author + touched record ids, the oplog row is
        // stamped with the original author (not the relay `source`) and the record ids.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut relay = store();
        // Importing source is the RELAY (peer:A); original author is peer:C.
        let imported = relay
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:A",
                Some("peer:C"),
                &["t1"],
                &idx,
            )
            .unwrap();
        assert!(imported, "a fresh chunk is newly imported");
        // The single-chunk import rebuilt the projection too (review 090 #3): the
        // imported record is visible, not stranded under a stale projection.
        assert_eq!(
            relay.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
            json!("Ship"),
            "delegating to apply_remote_chunks materializes the record"
        );

        let ops = relay.list_ops().unwrap();
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.kind, "record.remote_import");
        assert_eq!(op.actor_id, "peer:C", "oplog attributes the ORIGINAL author, not the relay");
        let payload: serde_json::Value = serde_json::from_slice(&op.payload).unwrap();
        assert_eq!(payload["source"], json!("peer:C"), "payload source is the original author");
        assert_eq!(
            payload["record_ids"],
            json!(["t1"]),
            "the touched record id is preserved (never a provenance-poor empty list)"
        );

        // Idempotent: a second identical import adds no oplog row.
        let again = relay
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:A",
                Some("peer:C"),
                &["t1"],
                &idx,
            )
            .unwrap();
        assert!(!again, "re-importing a present chunk is an idempotent no-op");
        assert_eq!(relay.list_ops().unwrap().len(), 1, "no duplicate oplog row");
    }

    #[test]
    fn legacy_put_chunk_from_remote_first_hop_attributes_its_source() {
        // The non-forwarded control: with no original author supplied, the importing
        // `source` IS the author, so the oplog records it (unchanged direct-import
        // behavior) — but still carries the touched record ids.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t9", json!({"title": "X"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        dst.put_chunk_from_remote(
            &doc_id,
            &row.chunk_id,
            &row.format,
            &row.payload,
            "peer:origin",
            None,
            &["t9"],
            &idx,
        )
        .unwrap();
        let ops = dst.list_ops().unwrap();
        assert_eq!(ops[0].actor_id, "peer:origin");
        let payload: serde_json::Value = serde_json::from_slice(&ops[0].payload).unwrap();
        assert_eq!(payload["source"], json!("peer:origin"));
        assert_eq!(payload["record_ids"], json!(["t9"]));
    }

    #[test]
    fn legacy_put_chunk_from_remote_rejects_empty_record_ids_without_touching_store() {
        // review 096: the public single-chunk import API must REJECT provenance-poor
        // input rather than write a record.remote_import row that names no record.
        // `record_ids = &[]` errors, and — because the guard runs BEFORE the
        // transaction — leaves the store completely unchanged (no chunk, no oplog row).
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        let chunks_before = total_chunk_rows(&dst);
        let ops_before = dst.list_ops().unwrap().len();

        let err = dst
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:A",
                Some("peer:C"),
                &[], // no touched record id -> provenance-poor, must be rejected
                &idx,
            )
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError", "empty record_ids is rejected");
        assert!(
            err.to_string().contains("no touched record id"),
            "error names the missing record id: {err}"
        );

        // The store is byte-for-byte unchanged: no chunk and no oplog row leaked in.
        assert_eq!(total_chunk_rows(&dst), chunks_before, "no chunk row was appended");
        assert_eq!(dst.list_ops().unwrap().len(), ops_before, "no oplog row was appended");
        assert!(
            dst.get_chunk(&doc_id, &row.chunk_id).unwrap().is_none(),
            "the rejected chunk did not land"
        );
    }

    #[test]
    fn legacy_put_chunk_from_remote_rejects_blank_record_id_entries() {
        // The blank-entry twin: `&[""]` (or whitespace-only ids) is just as
        // provenance-poor as `&[]`, so it is rejected the same way — no `&[""]`
        // loophole past the floor.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        let err = dst
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:A",
                Some("peer:C"),
                &["   "], // whitespace-only id is not a real record identity
                &idx,
            )
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError", "blank record id is rejected");
        assert_eq!(total_chunk_rows(&dst), 0, "no chunk landed for a blank record id");
        assert!(dst.list_ops().unwrap().is_empty(), "no oplog row for a blank record id");
    }

    #[test]
    fn legacy_put_chunk_from_remote_rejects_blank_author_without_touching_store() {
        // The author-floor twin: a blank effective original author (blank `source` and
        // no `author_actor_id`) would write a remote-import row attributable to no one,
        // so it is rejected before any store mutation.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        let err = dst
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "   ", // blank importing source
                None,  // and no forwarded original author
                &["t1"],
                &idx,
            )
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError", "blank author/source is rejected");
        assert!(
            err.to_string().contains("no original author/source"),
            "error names the missing author: {err}"
        );
        assert_eq!(total_chunk_rows(&dst), 0, "no chunk landed for a blank author");
        assert!(dst.list_ops().unwrap().is_empty(), "no oplog row for a blank author");
    }

    #[test]
    fn legacy_put_chunk_from_remote_rebuilds_projection_and_indexes_atomically() {
        // review 090 #3: the ONLY public single-chunk remote-import surface must not
        // be an escape hatch around the DL-4 atomic invariant. Before the fix it wrote
        // crdt_chunks + oplog but SKIPPED the projection/index rebuild, stranding the
        // imported record under a stale `records` table (and a stale FTS index). It now
        // delegates to `apply_remote_chunks`, so in ONE transaction the chunk lands,
        // the records projection materializes, AND the receiver's active FTS index is
        // rebuilt — proving there is no non-atomic public path left.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(
            &insert("tasks", "t1", json!({"title": "offline rebuild"}), 1),
            &idx,
        )
        .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        // The RECEIVER has its OWN active FTS index over tasks/f_title (review 084 #1:
        // index metadata is per-store, not part of the synced chunk payload).
        let mut dst = store();
        let mut dst_idx = IndexManager::new();
        dst.create_index(&mut dst_idx, "tasks", "f_title", CreateIndexKind::Fts)
            .expect("create receiver fts index");

        let imported = dst
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:origin",
                None,
                &["t1"],
                &dst_idx,
            )
            .unwrap();
        assert!(imported, "a fresh chunk is newly imported");

        // The chunk + a remote-tagged oplog row landed AND the projection materialized.
        assert_eq!(total_chunk_rows(&dst), 1);
        assert_eq!(dst.list_ops().unwrap().len(), 1);
        assert_eq!(
            dst.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
            json!("offline rebuild"),
            "the single-chunk import materialized the record (no stale projection)"
        );
        // The receiver's FTS index was rebuilt by the same atomic apply, so the
        // imported record is immediately searchable — the projection AND indexes are
        // consistent, not just crdt_chunks + oplog.
        assert_eq!(
            dst_idx
                .fts_match(dst.connection(), "tasks", "f_title", "offline")
                .unwrap(),
            vec!["t1".to_string()],
            "the single-chunk import rebuilt the receiver's FTS index (review 090 #3)"
        );
    }

    #[test]
    fn legacy_put_chunk_from_remote_rolls_back_entirely_on_rebuild_failure() {
        // review 090 #3 / 088 #1: because the single-chunk import now delegates to the
        // atomic `apply_remote_chunks`, a chunk whose bytes the in-transaction rebuild
        // rejects must roll the WHOLE import back — no stranded crdt_chunks row, no
        // stranded oplog row, and the prior projection untouched.
        let mut dst = store();
        let idx = IndexManager::new();
        dst.apply_mutation_crdt(&insert("tasks", "kept", json!({"title": "stays"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let chunks_before = total_chunk_rows(&dst);
        let ops_before = dst.list_ops().unwrap();
        let kept_before = dst.get_record("tasks", "kept").unwrap().unwrap();

        // Garbage Loro bytes under a fresh network-safe id: the append-only insert
        // accepts them (content-agnostic), but the in-transaction rebuild folds the
        // doc's chunks through `from_updates`, which rejects the garbage and fails.
        let err = dst
            .put_chunk_from_remote(
                &doc_id,
                "sha256:deadbeef",
                CHUNK_FORMAT,
                &[0xde, 0xad, 0xbe, 0xef],
                "peer:99",
                None,
                &["kept"],
                &idx,
            )
            .unwrap_err();
        assert_eq!(err.code(), "SyncError", "garbage chunk must fail the rebuild");

        // The whole single-chunk apply rolled back.
        assert_eq!(
            total_chunk_rows(&dst),
            chunks_before,
            "a failed single-chunk import must leave NO new crdt_chunks rows"
        );
        assert_eq!(
            dst.list_ops().unwrap(),
            ops_before,
            "a failed single-chunk import must leave NO new oplog rows"
        );
        assert!(
            dst.get_chunk(&doc_id, "sha256:deadbeef").unwrap().is_none(),
            "the garbage chunk must not persist"
        );
        assert_eq!(
            dst.get_record("tasks", "kept").unwrap().unwrap(),
            kept_before,
            "the prior record must be untouched after a rolled-back single-chunk import"
        );
    }

    #[test]
    fn first_hop_import_attributes_the_importing_source_as_author() {
        // The non-forwarded control: a chunk with no `author_actor_id` is a first-hop
        // import where the importing `source` IS the author. The oplog records that
        // source (unchanged behavior), so the provenance change does not regress the
        // direct-import path.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t9", json!({"title": "X"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let chunk = one_chunk(&src, &doc_id); // author_actor_id: None

        let mut dst = store();
        dst.apply_remote_chunks(std::slice::from_ref(&chunk), "peer:origin", &idx)
            .unwrap();
        let ops = dst.list_ops().unwrap();
        assert_eq!(ops[0].actor_id, "peer:origin", "first-hop import attributes its source");
        let payload: serde_json::Value = serde_json::from_slice(&ops[0].payload).unwrap();
        assert_eq!(payload["source"], json!("peer:origin"));
    }

    /// Review 088 #1 (P2): a remote apply that inserts chunks and THEN fails during
    /// the projection rebuild must roll back ENTIRELY — the receiving store must be
    /// left with NO new `crdt_chunks` rows, NO new `oplog` rows, and an unchanged
    /// `records` projection. Before the fix, per-chunk commits + a post-hoc rebuild
    /// could strand committed chunk/oplog rows under a stale projection.
    ///
    /// Injection: stage a VALID chunk (which inserts cleanly) followed by a GARBAGE
    /// chunk for the SAME collection doc. The garbage bytes insert into `crdt_chunks`
    /// fine (the append-only insert is content-agnostic), but the in-transaction
    /// `rebuild_projection` then folds the doc's chunks back through Loro
    /// `from_updates`, which rejects the garbage with a `SyncError`. That error must
    /// roll the whole apply back.
    #[test]
    fn apply_remote_chunks_rolls_back_entirely_on_rebuild_failure() {
        // Seed the receiver with a prior LOCAL record so there is committed history
        // and a projection row that must survive the failed remote apply untouched.
        let mut dst = store();
        let idx = IndexManager::new();
        dst.apply_mutation_crdt(&insert("tasks", "kept", json!({"title": "stays"}), 1), &idx)
            .unwrap();

        let doc_id = collection_doc_id("tasks");
        let chunks_before = total_chunk_rows(&dst);
        let ops_before = dst.list_ops().unwrap();
        let kept_before = dst.get_record("tasks", "kept").unwrap().unwrap();

        // A genuinely valid foreign chunk (a record `t2` from another store) staged
        // BEFORE a garbage chunk under a fresh id for the same doc. Both ride under
        // network-safe content-style ids that do NOT collide with the receiver's
        // local `chunk-0001`, so the GOOD chunk inserts cleanly and the failure is
        // injected purely at the in-transaction rebuild (not the append-only guard).
        let mut other = store();
        other
            .apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "remote"}), 2), &idx)
            .unwrap();
        let mut good = one_chunk(&other, &doc_id);
        good.chunk_id = "sha256:goodgood".to_string();
        let garbage = RemoteChunk {
            doc_id: doc_id.clone(),
            chunk_id: "sha256:deadbeef".to_string(),
            format: CHUNK_FORMAT.to_string(),
            payload: vec![0xde, 0xad, 0xbe, 0xef],
            author_actor_id: None,
            record_ids: Vec::new(),
        };

        let err = dst
            .apply_remote_chunks(&[good, garbage], "peer:99", &idx)
            .unwrap_err();
        assert_eq!(err.code(), "SyncError", "garbage chunk must fail the rebuild");

        // Whole apply rolled back: no new chunk rows, no new oplog rows, projection
        // byte-for-byte as before (the good chunk did NOT leak in).
        assert_eq!(
            total_chunk_rows(&dst),
            chunks_before,
            "a failed remote apply must leave NO new crdt_chunks rows"
        );
        assert_eq!(
            dst.list_ops().unwrap(),
            ops_before,
            "a failed remote apply must leave NO new oplog rows"
        );
        assert!(
            dst.get_chunks(&doc_id).unwrap().iter().all(|c| c.chunk_id != "sha256:deadbeef"),
            "the garbage chunk must not persist"
        );
        assert!(
            dst.get_record("tasks", "t2").unwrap().is_none(),
            "the staged good record must not leak through a rolled-back apply"
        );
        assert_eq!(
            dst.get_record("tasks", "kept").unwrap().unwrap(),
            kept_before,
            "the prior record must be untouched after a rolled-back apply"
        );
    }

    // --- DL-6: rebuild equals maintained projection (zero diff) -----------

    #[test]
    fn rebuild_projection_equals_maintained_projection() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("tasks", "t1", json!({"done": true}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "B"}), 3), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 4), &idx).unwrap();
        s.apply_mutation_crdt(&insert("notes", "n1", json!({"body": "hi"}), 5), &idx)
            .unwrap();

        let before = projection_snapshot(&s);
        s.rebuild_projection(&idx).unwrap();
        let after = projection_snapshot(&s);
        assert_eq!(after, before, "DL-6 rebuild must be byte-identical to the maintained projection");
        // The deleted record stays gone after rebuild (no tombstone resurrection).
        assert!(!after.contains_key("tasks/t1"));
        assert!(after.contains_key("tasks/t2"));
        assert!(after.contains_key("notes/n1"));
    }

    #[test]
    fn reinsert_after_delete_rebuilds_recreated_record() {
        // The hardest DL-21 case: insert -> delete -> reinsert same id. Rebuild must
        // show the recreated record, not a lingering tombstone.
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Old"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t1", 2), &idx).unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "New"}), 3), &idx)
            .unwrap();
        let before = projection_snapshot(&s);
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap().fields["title"], json!("New"));
        s.rebuild_projection(&idx).unwrap();
        assert_eq!(projection_snapshot(&s), before);
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap().fields["title"], json!("New"));
    }

    // --- Fixture corpus (fixtures/crdt-write, T024) ----------------------

    #[derive(serde::Deserialize)]
    struct FixtureOp {
        op: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        fields: serde_json::Value,
        #[serde(default)]
        items: Vec<FixtureOp>,
    }

    #[derive(serde::Deserialize)]
    struct Fixture {
        case: String,
        collection: String,
        doc_id: String,
        ops: Vec<FixtureOp>,
        expect_records: Vec<serde_json::Value>,
        #[serde(default)]
        expect_deleted_ids: Vec<String>,
        expect_chunk_count: usize,
        #[serde(default)]
        expect_oplog_kinds: Vec<String>,
        #[serde(default)]
        rebuild_chunk_order: Option<Vec<String>>,
        rebuild_equals_projection: bool,
    }

    /// Convert a fixture op into a storage `Mutation`, threading a per-op logical
    /// clock so timestamps advance deterministically.
    fn op_to_mutation(collection: &str, op: &FixtureOp, at: i64) -> Mutation {
        match op.op.as_str() {
            "insert" => insert(collection, op.id.as_deref().unwrap(), op.fields.clone(), at),
            "patch" => patch(collection, op.id.as_deref().unwrap(), op.fields.clone(), at),
            "delete" => delete(collection, op.id.as_deref().unwrap(), at),
            "transact" => Mutation::Transact {
                items: op
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, child)| op_to_mutation(collection, child, at + i as i64))
                    .collect(),
            },
            other => panic!("unknown fixture op {other}"),
        }
    }

    fn load_fixture(name: &str) -> Fixture {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/crdt-write")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
    }

    fn run_fixture(name: &str) {
        let fx = load_fixture(name);
        let mut s = store();
        let idx = IndexManager::new();

        // Apply each top-level fixture op as one DL-4 logical write.
        let mut clock = 0i64;
        for op in &fx.ops {
            clock += 1;
            let m = op_to_mutation(&fx.collection, op, clock);
            match &m {
                Mutation::Transact { items } => {
                    clock += items.len() as i64;
                    s.transact_mutations_crdt(items, &idx).unwrap();
                }
                _ => s.apply_mutation_crdt(&m, &idx).unwrap(),
            }
        }

        // expect_records (visible, ordered).
        assert_eq!(
            visible_records(&s, &fx.collection),
            fx.expect_records,
            "case {}: visible projection mismatch",
            fx.case
        );

        // expect_deleted_ids: each must be absent from the live projection but
        // derivable from CRDT history (the doc's chunk history records the delete).
        for id in &fx.expect_deleted_ids {
            assert!(
                s.get_record(&fx.collection, id).unwrap().is_none()
                    || s.get_record(&fx.collection, id).unwrap().unwrap().deleted,
                "case {}: deleted id {id} must not be live",
                fx.case
            );
        }

        // expect_chunk_count.
        let chunks = s.get_chunks(&fx.doc_id).unwrap();
        assert_eq!(
            chunks.len(),
            fx.expect_chunk_count,
            "case {}: chunk count mismatch",
            fx.case
        );

        // expect_oplog_kinds (in order).
        if !fx.expect_oplog_kinds.is_empty() || fx.expect_chunk_count == 0 {
            let kinds: Vec<String> = s.list_ops().unwrap().into_iter().map(|o| o.kind).collect();
            assert_eq!(kinds, fx.expect_oplog_kinds, "case {}: oplog kinds mismatch", fx.case);
        }

        // DL-6: rebuild_equals_projection — drop and reconstruct purely from chunks.
        if fx.rebuild_equals_projection {
            let before = projection_snapshot(&s);
            s.rebuild_projection(&idx).unwrap();
            let after = projection_snapshot(&s);
            assert_eq!(
                after, before,
                "case {}: DL-6 rebuild must equal the maintained projection",
                fx.case
            );
        }

        // rebuild_chunk_order: rebuild a fresh doc from the persisted chunks in the
        // fixture-specified (shuffled/duplicated) order and assert the materialized
        // records still equal expect_records — pins Loro's order/dup independence.
        if let Some(order) = &fx.rebuild_chunk_order {
            let by_id: std::collections::HashMap<String, Vec<u8>> = chunks
                .iter()
                .map(|c| (c.chunk_id.clone(), c.payload.clone()))
                .collect();
            let payloads: Vec<Vec<u8>> = order
                .iter()
                .map(|cid| by_id.get(cid).expect("fixture chunk id present").clone())
                .collect();
            let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_slice()).collect();
            let rebuilt = RecordsDoc::from_updates(LOCAL_PEER_ID, &refs).unwrap();
            let mut visible: Vec<serde_json::Value> = rebuilt
                .list_record_ids()
                .into_iter()
                .filter_map(|id| {
                    let env: RecordEnvelope =
                        serde_json::from_value(rebuilt.get_record(&id)?).ok()?;
                    if env.deleted {
                        return None;
                    }
                    Some(json!({
                        "id": env.entity_id.as_str(),
                        "fields": serde_json::to_value(&env.fields).unwrap(),
                    }))
                })
                .collect();
            visible.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            assert_eq!(
                visible, fx.expect_records,
                "case {}: out-of-order/duplicate chunk rebuild must converge",
                fx.case
            );
        }
    }

    #[test]
    fn fixture_insert_read_rebuild() {
        run_fixture("insert_read_rebuild.json");
    }
    #[test]
    fn fixture_patch_preserves_omitted_fields() {
        run_fixture("patch_preserves_omitted_fields.json");
    }
    #[test]
    fn fixture_delete_tombstone_rebuild() {
        run_fixture("delete_tombstone_rebuild.json");
    }
    #[test]
    fn fixture_insert_patch_delete_rebuild() {
        run_fixture("insert_patch_delete_rebuild.json");
    }
    #[test]
    fn fixture_two_records_independent_rebuild() {
        run_fixture("two_records_independent_rebuild.json");
    }
    #[test]
    fn fixture_reinsert_after_delete_rebuild() {
        run_fixture("reinsert_after_delete_rebuild.json");
    }
    #[test]
    fn fixture_unknown_forward_compat_preserved() {
        run_fixture("unknown_forward_compat_preserved.json");
    }
    #[test]
    fn fixture_empty_collection_rebuild() {
        run_fixture("empty_collection_rebuild.json");
    }
    #[test]
    fn fixture_transact_group_single_chunk() {
        run_fixture("transact_group_single_chunk.json");
    }
    #[test]
    fn fixture_rebuild_duplicate_out_of_order_chunks() {
        run_fixture("rebuild_duplicate_out_of_order_chunks.json");
    }
}
