//! DL-6 projection rebuild: drop the entire `records` projection and reconstruct
//! it purely from `crdt_chunks` (the CRDT source of truth), then rebuild active
//! indexes — equal to the maintained projection with zero diff.

use super::crdt_encoding::{envelope_from_doc, load_doc_tx};
use crate::index::IndexManager;
use crate::{map_json, map_sql, put_record_tx, Store};
use forge_crdt::RecordsDoc;
use forge_domain::Result;
use rusqlite::params;

impl Store {
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
}

/// DL-6 projection rebuild inside a caller-provided transaction: drop the whole
/// `records` projection, re-materialize it from `crdt_chunks` (the CRDT source of
/// truth), and rebuild active physical indexes — all on `tx` so the rebuild commits
/// or rolls back with whatever else the caller did in the same transaction. The
/// shared engine behind [`Store::rebuild_projection`] and
/// [`Store::apply_remote_chunks`].
pub(crate) fn rebuild_projection_tx(
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
pub(super) fn materialize_record_into_projection(
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

/// The collection name encoded in a `collection/<name>` doc id, or `None` if the
/// doc id is not a records-collection doc.
///
/// Public so the sync seam (and the SS-7 authorization gate above it) can derive
/// the target collection of an incoming chunk straight from its `doc_id` without
/// re-parsing the prefix, mirroring [`collection_doc_id`](super::collection_doc_id).
pub fn collection_of_doc(doc_id: &str) -> Option<&str> {
    doc_id.strip_prefix("collection/")
}
