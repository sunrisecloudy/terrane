//! The DL-4 remote-import path: append a peer's chunks (append-only, idempotent)
//! with a provenance-preserving `record.remote_import` oplog row, then rebuild the
//! projection + indexes — all in ONE transaction so a failure rolls back wholly
//! (review 088 #1 / 090 #3 / 092 #1/#2).

use super::chunk_storage::OptionalRow;
use super::oplog::{append_op_tx, chunk_id_lamport, OplogPayload};
use super::rebuild::rebuild_projection_tx;
use crate::index::IndexManager;
use crate::kv::{kv_get_tx, kv_set_tx};
use crate::migration::{advance_schema_version_if_newer_tx, META_NS, SCHEMA_REGISTRY_KEY};
use crate::{map_sql, now_ms, Store};
use forge_domain::{CoreError, Result};
use forge_schema::{CollectionDef, SchemaRegistry};
use rusqlite::params;

impl Store {
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
    /// `Some(to)` when this chunk is a DL-13 **migration** chunk carrying the
    /// schema-version advance it authored: the receiver advances its persisted
    /// `schema_version` to `to` IN THE SAME txn as the chunk import + rebuild, so a
    /// peer that materializes the migrated record values can never stay behind at the
    /// old `schema_version` (review 139). `None` for an ordinary record-write chunk,
    /// which leaves `schema_version` untouched. Recovered by the sync seam from the
    /// origin's per-chunk migration oplog row; never widens authorization (the chunk
    /// is still gated as a record write against its `record_ids`).
    pub schema_version: Option<u64>,
    /// `Some(entry)` when this DL-13 migration chunk also carries the affected
    /// collection's EVOLVED registry entry (a serialized `forge_schema::CollectionDef`),
    /// so an authorized receiver evolves its `SchemaRegistry` IN THE SAME txn as the
    /// chunk import + `schema_version` advance — the registry is a CRDT document that
    /// syncs with the migration, never drifting behind the data it describes
    /// (prd-merged/02:15, review 143). Recovered by the sync seam from the per-chunk
    /// migration oplog row's `registry_collection` field. `None` for an ordinary
    /// record-write chunk (and for a migration driven without a registry change). It
    /// never widens authorization: the caller authorizes the migration as a schema
    /// change (Owner/Maintainer + `schema_write`) BEFORE the chunk is staged for import.
    pub registry_collection: Option<serde_json::Value>,
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
    let RemoteChunk {
        doc_id,
        chunk_id,
        format,
        payload,
        author_actor_id,
        record_ids,
        schema_version,
        registry_collection,
    } = chunk;
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
    // Same `OplogPayload` builder as the local write path so the payload schema
    // cannot skew across the two sites; the remote variant carries `source` (the
    // original author) and no `collection`, preserving the prior shape exactly.
    //
    // Review 145 — multi-hop relay: when this chunk is a MIGRATION chunk
    // (`schema_version.is_some()`), thread its schema-affecting metadata (the target
    // `to` version + the evolved `registry_collection`) THROUGH this remote-import row.
    // Otherwise the row would record only `record.remote_import` with no version/registry,
    // and when THIS store relays the chunk to the next hop the sync seam would see a plain
    // record write, import the migrated data, and never advance that hop's schema_version /
    // registry. Carrying the metadata makes a migration a schema-affecting op at EVERY hop
    // (and lets the seam re-authorize schema_write at each one). An ordinary record import
    // passes `None`/`None`, so its row is byte-identical to before.
    let op_payload = OplogPayload::remote_import(
        doc_id,
        chunk_id,
        "record.remote_import",
        original_author,
        record_ids.clone(),
        *schema_version,
        registry_collection.clone(),
    )
    .encode("remote oplog payload encode")?;
    append_op_tx(
        tx,
        &op_id,
        original_author,
        "remote",
        lamport,
        "record.remote_import",
        &op_payload,
    )?;

    // DL-13 review 139: a MIGRATION chunk carries the schema-version advance it
    // authored. On its authorized import, bump the RECEIVING store's persisted
    // `schema_version` to the carried target IN THIS SAME txn (alongside the chunk
    // insert + the projection/index rebuild the caller runs in the same
    // `Store::transact`), so a receiver can never materialize migrated record values
    // while staying behind at the old version. The advance is monotone and
    // idempotent: a receiver already at or beyond the target is left unchanged (a
    // converged peer, or one that authored the same migration locally), never an
    // error — so a re-sync stays a pure no-op.
    if let Some(to) = schema_version {
        // Advance the RECEIVER's workspace-global `schema_version` toward the carried
        // target, monotonically and idempotently (a peer already at/beyond the target is
        // left unchanged — never an error — so a re-sync stays a pure no-op).
        advance_schema_version_if_newer_tx(tx, *to)?;

        // DL-13 review 143/145 (review-w9 P1): a migration chunk ALSO carries the
        // affected collection's evolved registry entry; evolve the receiver's persisted
        // `SchemaRegistry` IN THIS SAME txn so the registry never drifts behind the
        // migrated data. This merge is keyed PER-COLLECTION and must NOT be gated on the
        // GLOBAL `schema_version` actually moving forward: `schema_version` is one
        // workspace-wide counter, so a receiver whose global version already reached the
        // target via UNRELATED schema work on OTHER collections would otherwise import
        // this collection's migrated records while skipping its registry entry — leaving
        // data ahead of schema (the exact drift class review 143 closed). `sync_collection`
        // is a replace-or-insert + re-validate, so applying it on a converged / already-
        // migrated peer is a safe idempotent no-op; a malformed carried entry (or a
        // re-validation failure) rolls the whole import back together with the chunk +
        // version, so the receiver never persists a version ahead of its registry.
        if let Some(registry_collection) = registry_collection {
            evolve_registry_collection_tx(tx, registry_collection)?;
        }
    }
    Ok(true)
}

/// Evolve the receiver's persisted `SchemaRegistry` with the affected collection's
/// EVOLVED entry carried by an authorized DL-13 migration chunk, IN THE caller's
/// import transaction (review 143). Reads the persisted registry JSON (default empty),
/// deserializes the carried `CollectionDef`, merges it via
/// [`SchemaRegistry::sync_collection`] (replace-or-insert + re-validate), and writes
/// the merged registry back under `__forge/meta`/`schema_registry`.
///
/// Because this runs inside the same transaction as the chunk insert + the
/// `schema_version` advance, the registry, the records, and the version commit (or
/// roll back) TOGETHER — no drift window. A malformed carried entry (or a registry
/// that fails re-validation) returns a typed error that rolls the whole import back,
/// so the receiver never persists a structurally-invalid schema or a version ahead of
/// its registry. The migration was already authorized as a schema change
/// (Owner/Maintainer + `schema_write`) before its chunk was staged, so this seam is
/// reached only for a trusted, schema-authorized peer.
fn evolve_registry_collection_tx(
    tx: &rusqlite::Transaction<'_>,
    registry_collection: &serde_json::Value,
) -> Result<()> {
    // The carried entry is `{ "name": <collection>, "collection": <CollectionDef> }`
    // so the receiver knows which collection to replace without re-deriving the name.
    let name = registry_collection
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CoreError::SchemaCompatibilityError(
                "migration registry_collection is missing the collection name".into(),
            )
        })?;
    let collection_value = registry_collection.get("collection").ok_or_else(|| {
        CoreError::SchemaCompatibilityError(
            "migration registry_collection is missing the collection definition".into(),
        )
    })?;
    let collection: CollectionDef = serde_json::from_value(collection_value.clone())
        .map_err(|e| {
            CoreError::SchemaCompatibilityError(format!(
                "migration registry_collection is not a valid collection definition: {e}"
            ))
        })?;

    // Load the persisted registry (default empty for a fresh receiver), re-validating
    // it so a corrupt persisted registry surfaces here rather than being silently
    // merged into.
    let mut registry = match kv_get_tx(tx, META_NS, SCHEMA_REGISTRY_KEY)? {
        Some(bytes) => {
            let parsed: SchemaRegistry = serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("deserialize schema registry on sync apply: {e}"))
            })?;
            parsed.validated()?
        }
        None => SchemaRegistry::new(),
    };
    registry.sync_collection(name, collection)?;

    let bytes = serde_json::to_vec(&registry).map_err(|e| {
        CoreError::StorageError(format!("serialize schema registry on sync apply: {e}"))
    })?;
    kv_set_tx(tx, META_NS, SCHEMA_REGISTRY_KEY, &bytes, "application/json")
}
