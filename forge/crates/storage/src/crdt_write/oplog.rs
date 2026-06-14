//! Oplog metadata for the CRDT write path (DL-4): the logical-mutation `kind`
//! string, the tx-scoped oplog-row append, and the chunk→lamport derivation that
//! keeps the oplog's `(lamport, op_id)` total order matching write order.

use crate::{map_json, map_sql, now_ms, Mutation};
use forge_domain::Result;
use rusqlite::params;

/// The single source of truth for the `oplog.payload` JSON written by BOTH the
/// local group-write path ([`super::mutation`]) and the remote-import path
/// ([`super::remote::import_remote_chunk_tx`]). Both paths once hand-built this
/// object inline; the two copies could skew the field set / encoding, yet the sync
/// seam ([`forge-sync`]'s `oplog_index`) and the SS-7 relay-hop recovery read these
/// fields back by key (`record_ids`, `source`). Funnelling both through this one
/// builder makes the on-disk schema un-skewable.
///
/// The two variants legitimately differ in their payload shape (this is the prior
/// behavior, preserved byte-for-byte — NOT a unification of the field set):
///
/// * a **local** write carries `collection` and NO `source`;
/// * a **remote import** carries `source` (the original author) and NO `collection`.
///
/// Bytes are produced via [`serde_json::Value`] (a `serde_json::Map`, i.e. a
/// `BTreeMap` since `preserve_order` is off), so the keys serialize in alphabetical
/// order regardless of construction order — identical to the prior inline `json!`.
pub(super) struct OplogPayload {
    doc_id: String,
    chunk_id: String,
    kind: String,
    record_ids: Vec<String>,
    /// Present only on the LOCAL write path (the source collection name).
    collection: Option<String>,
    /// Present only on the REMOTE-import path (the chunk's original author).
    source: Option<String>,
}

impl OplogPayload {
    /// The payload for a LOCAL group write: `{chunk_id, collection, doc_id, kind,
    /// record_ids}` (alphabetical on the wire). No `source` key — preserving the
    /// prior local-path shape exactly.
    pub(super) fn local(
        doc_id: &str,
        chunk_id: &str,
        collection: &str,
        kind: &str,
        record_ids: Vec<String>,
    ) -> Self {
        OplogPayload {
            doc_id: doc_id.to_string(),
            chunk_id: chunk_id.to_string(),
            kind: kind.to_string(),
            record_ids,
            collection: Some(collection.to_string()),
            source: None,
        }
    }

    /// The payload for a REMOTE import: `{chunk_id, doc_id, kind, record_ids,
    /// source}` (alphabetical on the wire). `source` is the original author so a
    /// later relay hop preserves provenance (`review 092 #1`); no `collection` key —
    /// preserving the prior remote-path shape exactly.
    pub(super) fn remote_import(
        doc_id: &str,
        chunk_id: &str,
        kind: &str,
        source: &str,
        record_ids: Vec<String>,
    ) -> Self {
        OplogPayload {
            doc_id: doc_id.to_string(),
            chunk_id: chunk_id.to_string(),
            kind: kind.to_string(),
            record_ids,
            collection: None,
            source: Some(source.to_string()),
        }
    }

    /// Encode to the `oplog.payload` bytes. Builds a `serde_json::Value` map (so the
    /// keys land in BTreeMap/alphabetical order, byte-identical to the prior inline
    /// `serde_json::json!`) and only emits `collection`/`source` when set.
    pub(super) fn encode(&self, context: &'static str) -> Result<Vec<u8>> {
        let mut map = serde_json::Map::new();
        map.insert("doc_id".into(), self.doc_id.as_str().into());
        map.insert("chunk_id".into(), self.chunk_id.as_str().into());
        map.insert("kind".into(), self.kind.as_str().into());
        if let Some(collection) = &self.collection {
            map.insert("collection".into(), collection.as_str().into());
        }
        if let Some(source) = &self.source {
            map.insert("source".into(), source.as_str().into());
        }
        map.insert("record_ids".into(), self.record_ids.clone().into());
        serde_json::to_vec(&serde_json::Value::Object(map)).map_err(|e| map_json(context, e))
    }
}

/// The stable oplog `kind` string for a logical mutation, e.g. `record.insert`.
/// Matches the fixtures' `expect_oplog_kinds`.
pub(super) fn oplog_kind(m: &Mutation) -> &'static str {
    match m {
        Mutation::Insert { .. } => "record.insert",
        Mutation::Update { .. } => "record.update",
        Mutation::Patch { .. } => "record.patch",
        Mutation::Delete { .. } => "record.delete",
        Mutation::Transact { .. } => "record.transact",
    }
}

/// Append one oplog row inside an open transaction (the DL-4 write metadata that
/// identifies the logical mutation, its doc id, and the chunk it produced). The
/// `op_id` is `(doc_id)#(chunk_id)`, unique because chunk ids are unique per doc.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_op_tx(
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

/// Derive a monotone-ish lamport for the oplog from the chunk sequence number, so
/// the oplog's `(lamport, op_id)` total order matches write order without a
/// separate clock. `chunk-0007` → lamport 7. Malformed ids fall back to 0.
pub(super) fn chunk_id_lamport(chunk_id: &str) -> u64 {
    chunk_id
        .strip_prefix("chunk-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}
