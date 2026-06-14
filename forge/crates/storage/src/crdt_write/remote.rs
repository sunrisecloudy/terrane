//! The DL-4 remote-import path: append a peer's chunks (append-only, idempotent)
//! with a provenance-preserving `record.remote_import` oplog row, then rebuild the
//! projection + indexes — all in ONE transaction so a failure rolls back wholly
//! (review 088 #1 / 090 #3 / 092 #1/#2).

use super::chunk_storage::OptionalRow;
use super::oplog::{append_op_tx, chunk_id_lamport, OplogPayload};
use super::rebuild::rebuild_projection_tx;
use crate::index::IndexManager;
use crate::{map_sql, now_ms, Store};
use forge_domain::{CoreError, Result};
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
    let op_payload = OplogPayload::remote_import(
        doc_id,
        chunk_id,
        "record.remote_import",
        original_author,
        record_ids.clone(),
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
        crate::migration::advance_schema_version_if_newer_tx(tx, *to)?;
    }
    Ok(true)
}
