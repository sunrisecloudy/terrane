//! Bridging the persisted `crdt_chunks` and the in-memory [`RecordsDoc`]: load /
//! reconstruct a doc from chunks (DL-4 step 3 / DL-6 rebuild primitive), and the
//! per-record envelope decode/encode that keeps the doc value byte-for-byte equal
//! to the materialized projection (so rebuild reproduces it exactly).

use crate::{map_json, map_sql};
use forge_crdt::RecordsDoc;
use forge_domain::{RecordEnvelope, Result};
use rusqlite::params;

/// Load (or reconstruct) a collection's `RecordsDoc` from its persisted chunks,
/// reading inside the open transaction so it sees only committed history (DL-4
/// step 3 / DL-6 rebuild primitive). An empty chunk set yields a fresh document.
pub(super) fn load_doc_tx(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    peer_id: u64,
) -> Result<RecordsDoc> {
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
pub(super) fn envelope_from_doc(doc: &RecordsDoc, id: &str) -> Result<Option<RecordEnvelope>> {
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
pub(super) fn write_envelope_to_doc(doc: &RecordsDoc, env: &RecordEnvelope) -> Result<()> {
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
