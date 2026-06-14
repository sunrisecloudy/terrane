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
    /// Present only on the LOCAL write path AND the MIGRATION path (the source
    /// collection name).
    collection: Option<String>,
    /// Present only on the REMOTE-import path (the chunk's original author).
    source: Option<String>,
    /// Present on the MIGRATION path AND on a REMOTE-import of a migration chunk: the
    /// schema-version pair the migration chunk advances `from → to` (DL-13). Threaded
    /// into the per-chunk oplog row so the sync seam can recover the receiver's target
    /// `schema_version` (review 139), AND — on a relay's `record.remote_import` row —
    /// so the NEXT hop still sees the chunk as schema-affecting (review 145). The local
    /// migration path knows both `from` and `to`; a relay re-export only needs to carry
    /// `to` forward, so the `from` it records is cosmetic (`0` when unknown).
    migration: Option<(u64, u64)>,
    /// Present on the MIGRATION path AND on a REMOTE-import of a migration chunk: the
    /// affected collection's EVOLVED registry entry (a serialized
    /// `forge_schema::CollectionDef`), carried so an authorized receiver evolves its
    /// `SchemaRegistry` in lockstep with the migrated records + `schema_version` — the
    /// registry is a CRDT document that syncs with the migration (prd-merged/02:15,
    /// review 143). Threaded into the per-chunk oplog row and recovered by the sync
    /// seam; preserved verbatim onto a relay's `record.remote_import` row so it survives
    /// the next hop (review 145). An opaque `serde_json::Value` so storage stays
    /// agnostic of the schema type's shape. `None` for an ordinary record write (and a
    /// registry-less migration).
    registry_collection: Option<serde_json::Value>,
    /// `true` ONLY on a REMOTE-import row of a MIGRATION chunk (review 145): an explicit
    /// "this forwarded chunk is schema-affecting" marker so a relaying peer re-stages it
    /// as a migration EVEN when it carried no `registry_collection` (a registry-less
    /// migration). The local-write / local-migration paths leave this `false` — they are
    /// distinguished by their oplog `kind` (`schema.migration`), not this flag. Emitted
    /// as `is_migration: true` so the sync seam can FAIL CLOSED if a remote-import row is
    /// marked schema-affecting but its `to` is unrecoverable, rather than importing
    /// migrated data as a plain record write that never advances the next hop's schema.
    remote_is_migration: bool,
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
            migration: None,
            registry_collection: None,
            remote_is_migration: false,
        }
    }

    /// The payload for a DL-13 **migration** chunk's per-chunk oplog row, keyed
    /// `(doc_id)#(chunk_id)` exactly like a local write so the sync seam discovers it
    /// by the SAME `missing_chunks_for_doc` join (review 139). It carries the migrated
    /// `record_ids` (so the chunk authorizes as a record write against concrete ids,
    /// not an empty list the RBAC gate denies), the `from`/`to` schema versions (so a
    /// receiver advances its `schema_version` to `to` on import), and the affected
    /// collection's EVOLVED registry entry `registry_collection` (so an authorized
    /// receiver evolves its `SchemaRegistry` in lockstep — review 143). Shape on the
    /// wire: `{chunk_id, collection, doc_id, from, kind, record_ids, registry_collection,
    /// to}` (alphabetical; `registry_collection` omitted when `None`).
    // The per-chunk migration row carries enough metadata for the sync seam to
    // authorize + apply it (ids, version pair, evolved registry entry); a struct of
    // args would not improve a single internal call site (mirrors `append_op_tx`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn migration(
        doc_id: &str,
        chunk_id: &str,
        collection: &str,
        kind: &str,
        record_ids: Vec<String>,
        from: u64,
        to: u64,
        registry_collection: Option<serde_json::Value>,
    ) -> Self {
        OplogPayload {
            doc_id: doc_id.to_string(),
            chunk_id: chunk_id.to_string(),
            kind: kind.to_string(),
            record_ids,
            collection: Some(collection.to_string()),
            source: None,
            migration: Some((from, to)),
            registry_collection,
            remote_is_migration: false,
        }
    }

    /// The payload for a REMOTE import: `{chunk_id, doc_id, kind, record_ids,
    /// source}` (alphabetical on the wire). `source` is the original author so a
    /// later relay hop preserves provenance (`review 092 #1`); no `collection` key —
    /// preserving the prior remote-path shape exactly.
    ///
    /// When the imported chunk is a DL-13 **migration** chunk (review 145), the
    /// schema-affecting metadata is carried THROUGH this row so the NEXT relay hop still
    /// sees a migration: `schema_version = Some(to)` emits `to` + `is_migration: true`,
    /// and `registry_collection` emits the evolved registry entry. Without this, a
    /// relay's `record.remote_import` row dropped the version/registry and the next hop
    /// imported the migrated data as a plain record write that never advanced its schema
    /// — the multi-hop bug this closes. An ordinary record import passes `None`/`None`
    /// and the row's shape is byte-identical to before.
    pub(super) fn remote_import(
        doc_id: &str,
        chunk_id: &str,
        kind: &str,
        source: &str,
        record_ids: Vec<String>,
        schema_version: Option<u64>,
        registry_collection: Option<serde_json::Value>,
    ) -> Self {
        OplogPayload {
            doc_id: doc_id.to_string(),
            chunk_id: chunk_id.to_string(),
            kind: kind.to_string(),
            record_ids,
            collection: None,
            source: Some(source.to_string()),
            // A relay only needs to carry `to` forward; the `from` is cosmetic (`0`).
            migration: schema_version.map(|to| (0, to)),
            registry_collection,
            remote_is_migration: schema_version.is_some(),
        }
    }

    /// Encode to the `oplog.payload` bytes. Builds a `serde_json::Value` map (so the
    /// keys land in BTreeMap/alphabetical order, byte-identical to the prior inline
    /// `serde_json::json!`) and only emits the keys the variant carries
    /// (`collection`/`source`/`from`/`to`).
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
        if let Some((from, to)) = self.migration {
            // The schema-version pair the migration chunk advances; the sync seam
            // reads `to` back to advance a receiver's `schema_version` (review 139). On a
            // relay's remote-import row `from` is cosmetic `0` — only `to` is recovered.
            map.insert("from".into(), from.into());
            map.insert("to".into(), to.into());
        }
        if self.remote_is_migration {
            // Explicit "this forwarded chunk is schema-affecting" marker on a
            // `record.remote_import` row (review 145), so a relaying peer re-stages it as
            // a migration even when it carried no `registry_collection`, and the sync seam
            // can FAIL CLOSED if the marker is set but `to` is unrecoverable.
            map.insert("is_migration".into(), true.into());
        }
        if let Some(registry_collection) = &self.registry_collection {
            // The affected collection's evolved registry entry; the sync seam reads it
            // back so an authorized receiver evolves its SchemaRegistry in lockstep with
            // the migrated records + schema_version (review 143). Omitted when absent.
            map.insert("registry_collection".into(), registry_collection.clone());
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
