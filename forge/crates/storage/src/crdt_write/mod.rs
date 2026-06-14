//! forge-storage CRDT-backed record write path (DL-4) + projection rebuild (DL-6).
//!
//! Normative spec: `prd-merged/02-data-layer-prd.md` DL-1..6/DL-17/DL-21 and
//! `forge/spec/crdt-write-path.md`. This module makes the **CRDT docs the source
//! of truth** and the `records` table a *derived, rebuildable projection*:
//!
//! - Each collection is one [`RecordsDoc`](forge_crdt::RecordsDoc) addressed by
//!   `doc_id = "collection/<name>"` (see [`collection_doc_id`]). The Loro map keys
//!   are record ids and each value is the record's **full serialized
//!   [`RecordEnvelope`](forge_domain::RecordEnvelope)** — so materializing the
//!   projection is "read the doc, write the row", which is exactly what rebuild
//!   does, giving zero diff by construction.
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
//!
//! This module is split into directory sub-modules (/simplify #8) — `mutation`,
//! `remote`, `rebuild`, `crdt_encoding`, `chunk_storage`, and `oplog` — re-exported
//! here so the public + crate-internal surface stays byte-stable. The DL-4 single-
//! transaction mutation engine (`mutation::write_group_crdt`) and the atomic remote
//! apply (`remote::apply_remote_chunks`) each keep their whole chunk/oplog/projection
//! write inside ONE `Store::transact` closure.

mod chunk_storage;
mod crdt_encoding;
mod mutation;
mod oplog;
mod rebuild;
mod remote;

pub use mutation::{collection_doc_id, CHUNK_FORMAT, LOCAL_PEER_ID};
pub use rebuild::collection_of_doc;
pub use remote::RemoteChunk;

// Crate-internal helper other storage modules reach as `crate::crdt_write::<name>`
// (`compaction` reaches `rebuild_projection_tx`). Re-exported here so that path
// stays stable after the directory split (/simplify #8); not part of the public
// API. (`import_remote_chunk_tx` stays `pub(crate)` in `remote` for its sibling
// `apply_remote_chunks`; nothing imports it through this facade.)
pub(crate) use rebuild::rebuild_projection_tx;
// The DL-13 migration durability seam (review 138 P1): the migration driver in
// `crate::migration` rewrites the CRDT source of truth (chunks), not just the
// projection, so a DL-6 rebuild reproduces the migrated values. Encapsulated here
// because it composes the same load-doc/export-chunk/materialize primitives the
// mutation path uses.
pub(crate) use mutation::migrate_collection_records_crdt_tx;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexManager;
    use crate::{CreateIndexKind, Mutation, Query, QueryResult, Store};
    use forge_crdt::RecordsDoc;
    use forge_domain::RecordEnvelope;
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
    fn oplog_payload_bytes_are_pinned_for_local_and_remote_paths() {
        // The unified `OplogPayload` builder (oplog.rs) must reproduce the prior
        // hand-built JSON byte-for-byte. The two shapes legitimately differ — a
        // LOCAL write carries `collection` and no `source`; a REMOTE import carries
        // `source` (the original author) and no `collection` — and serialize in
        // alphabetical key order (serde_json `Map`/BTreeMap). These goldens are the
        // pre-dedup bytes; the sync seam + SS-7 relay-hop recovery read these keys.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let local = String::from_utf8(src.list_ops().unwrap()[0].payload.clone()).unwrap();
        assert_eq!(
            local,
            r#"{"chunk_id":"chunk-0001","collection":"tasks","doc_id":"collection/tasks","kind":"record.insert","record_ids":["t1"]}"#,
            "local oplog payload bytes drifted"
        );

        let doc_id = collection_doc_id("tasks");
        let mut chunk = one_chunk(&src, &doc_id);
        chunk.author_actor_id = Some("peer:C".to_string());
        chunk.record_ids = vec!["t1".to_string()];
        let mut dst = store();
        dst.apply_remote_chunks(&[chunk], "peer:A", &idx).unwrap();
        let remote = String::from_utf8(dst.list_ops().unwrap()[0].payload.clone()).unwrap();
        assert_eq!(
            remote,
            r#"{"chunk_id":"chunk-0001","doc_id":"collection/tasks","kind":"record.remote_import","record_ids":["t1"],"source":"peer:C"}"#,
            "remote-import oplog payload bytes drifted"
        );
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

    // --- DL-17 transact scope: multi-collection transact is UNSUPPORTED ----

    #[test]
    fn transact_spanning_collections_is_rejected_unsupported() {
        // reviews 131/132 + DL-17: a single `transact` that spans MORE THAN ONE
        // collection is REJECTED at the write boundary. It would be LOCALLY atomic,
        // but it persists one chunk/oplog row PER collection with no transaction-group
        // id, and `forge-sync` authorizes/applies each chunk INDEPENDENTLY (SS-7), so a
        // peer denied one collection would import a torn half-transaction. Until the
        // SS-7 boundary carries transaction-group metadata (DL-17 multi-collection-
        // atomic-SYNC, a separate future feature), a transact is confined to one
        // collection. The rejection is fail-closed: NEITHER collection is written.
        let mut s = store();
        let idx = IndexManager::new();
        // Prior good history in tasks that must survive untouched.
        s.apply_mutation_crdt(&insert("tasks", "keep", json!({"v": 1}), 1), &idx)
            .unwrap();
        let tasks_chunks_before = s.get_chunks(&collection_doc_id("tasks")).unwrap().len();
        let ops_before = s.list_ops().unwrap().len();

        // A two-collection group is rejected BEFORE any CRDT/SQLite work.
        let items = vec![
            insert("tasks", "t1", json!({"title": "A", "done": false}), 2),
            insert("notes", "n1", json!({"body": "hi"}), 3),
            patch("tasks", "t1", json!({"done": true}), 4),
        ];
        let err = s.transact_mutations_crdt(&items, &idx).unwrap_err();
        assert_eq!(err.code(), "QueryError");
        assert!(
            err.to_string().contains("multi-collection transact is not supported"),
            "typed rejection names the unsupported feature: {err}"
        );

        // Fail-closed: neither collection wrote a record, a chunk, or an oplog row.
        assert!(s.get_record("tasks", "t1").unwrap().is_none(), "tasks not written");
        assert!(s.get_record("notes", "n1").unwrap().is_none(), "notes not written");
        assert!(
            s.get_chunks(&collection_doc_id("notes")).unwrap().is_empty(),
            "no notes chunk for the rejected group"
        );
        assert_eq!(
            s.get_chunks(&collection_doc_id("tasks")).unwrap().len(),
            tasks_chunks_before,
            "no new tasks chunk for the rejected group"
        );
        assert_eq!(s.list_ops().unwrap().len(), ops_before, "no oplog row for the rejected group");
        // The prior single-collection history is untouched.
        assert_eq!(s.get_record("tasks", "keep").unwrap().unwrap().fields["v"], json!(1));
    }

    #[test]
    fn transact_single_collection_still_commits_atomically() {
        // The supported case: a single-collection `transact` still commits as ONE
        // atomic transaction — one doc, one chunk, one oplog row of kind
        // record.transact (byte-stable with the pre-scope path).
        let mut s = store();
        let idx = IndexManager::new();
        let items = vec![
            insert("tasks", "t1", json!({"title": "A", "done": false}), 1),
            insert("tasks", "t2", json!({"title": "B", "done": false}), 2),
            patch("tasks", "t1", json!({"done": true}), 3),
        ];
        let n = s.transact_mutations_crdt(&items, &idx).unwrap();
        assert_eq!(n, 3, "three leaves in one collection");
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap().fields["done"], json!(true));
        assert_eq!(s.get_record("tasks", "t2").unwrap().unwrap().fields["title"], json!("B"));
        assert_eq!(s.get_chunks(&collection_doc_id("tasks")).unwrap().len(), 1);
        let ops = s.list_ops().unwrap();
        assert_eq!(ops.len(), 1, "one oplog row for the single-collection group");
        assert_eq!(ops[0].kind, "record.transact");
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
            schema_version: None,
            registry_collection: None,
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
    fn migration_chunk_advances_receiver_schema_version_monotonically_and_atomically() {
        // Review 139: a RemoteChunk carrying `schema_version` (a DL-13 migration chunk)
        // must advance the RECEIVING store's persisted schema_version on import, in the
        // SAME txn as the chunk import + rebuild — and the advance is monotone (never
        // regresses) and idempotent (a re-import does not double-bump or error).
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("expenses");
        let mut chunk = one_chunk(&src, &doc_id);
        // Mark it a migration chunk to v2 (the version the migration advanced to).
        chunk.record_ids = vec!["e1".to_string()];
        chunk.schema_version = Some(2);

        let mut dst = store();
        assert_eq!(dst.schema_version().unwrap(), 1, "a fresh receiver starts at v1");
        dst.apply_remote_chunks(std::slice::from_ref(&chunk), "peer:7", &idx)
            .unwrap();
        assert_eq!(
            dst.schema_version().unwrap(),
            2,
            "importing a migration chunk advances the receiver to its target version"
        );
        // The migrated value materialized AND the version advance committed together.
        assert_eq!(dst.get_record("expenses", "e1").unwrap().unwrap().fields["amount"], json!(10));

        // Idempotent + monotone: re-importing the same chunk is a no-op; a chunk naming
        // an OLDER target version must NOT regress the receiver.
        dst.apply_remote_chunks(std::slice::from_ref(&chunk), "peer:7", &idx)
            .unwrap();
        assert_eq!(dst.schema_version().unwrap(), 2, "a re-import does not double-bump");
        let mut older = one_chunk(&src, &doc_id);
        older.chunk_id = "sha256:older".to_string();
        older.record_ids = vec!["e1".to_string()];
        older.schema_version = Some(1);
        dst.apply_remote_chunks(std::slice::from_ref(&older), "peer:7", &idx)
            .unwrap();
        assert_eq!(
            dst.schema_version().unwrap(),
            2,
            "a migration chunk naming an older version must not regress the receiver"
        );
    }

    #[test]
    fn migration_chunk_version_advance_rolls_back_with_a_failed_apply() {
        // Review 139 (atomicity): the receiver's schema_version advance must be bound to
        // the SAME txn as the chunk import + rebuild. A batch whose rebuild fails (a
        // garbage chunk) must roll the version advance back too — never leave the
        // receiver advanced while the migrated data was rolled out.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("expenses");
        let mut good = one_chunk(&src, &doc_id);
        good.chunk_id = "sha256:goodmigration".to_string();
        good.record_ids = vec!["e1".to_string()];
        good.schema_version = Some(2);
        // A garbage chunk for the same doc: it inserts (content-agnostic) but the
        // in-transaction rebuild rejects it, failing the whole batch.
        let garbage = RemoteChunk {
            doc_id: doc_id.clone(),
            chunk_id: "sha256:deadbeef".to_string(),
            format: CHUNK_FORMAT.to_string(),
            payload: vec![0xde, 0xad, 0xbe, 0xef],
            author_actor_id: None,
            record_ids: Vec::new(),
            schema_version: None,
            registry_collection: None,
        };

        let mut dst = store();
        assert_eq!(dst.schema_version().unwrap(), 1);
        let err = dst
            .apply_remote_chunks(&[good, garbage], "peer:99", &idx)
            .unwrap_err();
        assert_eq!(err.code(), "SyncError", "the garbage chunk fails the rebuild");
        assert_eq!(
            dst.schema_version().unwrap(),
            1,
            "a rolled-back migration import must NOT leave the receiver advanced"
        );
        assert!(
            dst.get_record("expenses", "e1").unwrap().is_none(),
            "the migrated record did not leak through a rolled-back apply"
        );
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
    fn legacy_put_chunk_from_remote_rejects_mixed_blank_and_valid_record_ids() {
        // review 097: the STRICT reject-on-blank contract. A list that mixes a blank
        // entry with a valid one (`&["", "t1"]`) used to pass the "some id is
        // non-blank" floor, then persist BOTH entries into the record.remote_import
        // row — so the next relay hop recovered a record_ids list containing an id
        // naming nothing. The boundary now rejects ANY blank entry outright and leaves
        // the store completely unchanged: no blank id is ever persisted.
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
                &["", "t1"], // a blank id mixed with a valid one is still rejected
                &idx,
            )
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError", "a blank entry is rejected");
        assert!(
            err.to_string().contains("no blank entries"),
            "error names the blank-free requirement: {err}"
        );

        // The store is byte-for-byte unchanged: not even the valid `t1` id leaked in.
        assert_eq!(total_chunk_rows(&dst), chunks_before, "no chunk row was appended");
        assert_eq!(dst.list_ops().unwrap().len(), ops_before, "no oplog row was appended");
        assert!(
            dst.get_chunk(&doc_id, &row.chunk_id).unwrap().is_none(),
            "the rejected chunk did not land"
        );
    }

    #[test]
    fn legacy_put_chunk_from_remote_trims_record_ids_in_persisted_row() {
        // review 097 positive case: a non-empty, blank-free list still imports, AND the
        // persisted record.remote_import row carries the ids in canonical (trimmed)
        // form, so every downstream hop recovers exactly the record identities — no
        // surrounding whitespace.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        let imported = dst
            .put_chunk_from_remote(
                &doc_id,
                &row.chunk_id,
                &row.format,
                &row.payload,
                "peer:A",
                Some("peer:C"),
                &["  t1  ", "t2"], // padded but valid ids
                &idx,
            )
            .unwrap();
        assert!(imported, "a valid non-empty list still imports");

        let ops = dst.list_ops().unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&ops[0].payload).unwrap();
        assert_eq!(
            payload["record_ids"],
            json!(["t1", "t2"]),
            "persisted record_ids are trimmed to canonical form, no blanks"
        );
    }

    #[test]
    fn legacy_put_chunk_from_remote_canonicalizes_persisted_author_and_source() {
        // review 101: the author-floor only TRIMMED the effective author for the
        // blank check, then handed the RAW `author_actor_id` and RAW `source` to the
        // shared import path — so a padded ` peer:C ` / ` peer:A ` passed validation
        // but persisted NON-canonical provenance as the oplog `actor_id` and the
        // record.remote_import payload `source`. Both are now trimmed up front, so the
        // persisted provenance is always canonical.

        // (a) forwarded chunk: a padded original author wins, trimmed in the row.
        let mut src = store();
        let idx = IndexManager::new();
        src.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 1), &idx)
            .unwrap();
        let doc_id = collection_doc_id("tasks");
        let row = src.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

        let mut dst = store();
        dst.put_chunk_from_remote(
            &doc_id,
            &row.chunk_id,
            &row.format,
            &row.payload,
            " peer:A ",        // padded first-hop source (the relay)
            Some(" peer:C "),  // padded forwarded original author
            &["t1"],
            &idx,
        )
        .unwrap();
        let ops = dst.list_ops().unwrap();
        assert_eq!(
            ops[0].actor_id, "peer:C",
            "persisted oplog actor_id is the trimmed original author"
        );
        let payload: serde_json::Value = serde_json::from_slice(&ops[0].payload).unwrap();
        assert_eq!(
            payload["source"],
            json!("peer:C"),
            "persisted payload source is the trimmed original author"
        );

        // (b) first-hop import (no forwarded author): the padded `source` itself is
        // the author, and it too is persisted in canonical (trimmed) form.
        let mut src2 = store();
        src2.apply_mutation_crdt(&insert("tasks", "t9", json!({"title": "X"}), 1), &idx)
            .unwrap();
        let doc_id2 = collection_doc_id("tasks");
        let row2 = src2.get_chunks(&doc_id2).unwrap().into_iter().next().unwrap();

        let mut dst2 = store();
        dst2.put_chunk_from_remote(
            &doc_id2,
            &row2.chunk_id,
            &row2.format,
            &row2.payload,
            " peer:origin ", // padded importing source IS the author
            None,
            &["t9"],
            &idx,
        )
        .unwrap();
        let ops2 = dst2.list_ops().unwrap();
        assert_eq!(
            ops2[0].actor_id, "peer:origin",
            "first-hop padded source is trimmed in the oplog actor_id"
        );
        let payload2: serde_json::Value = serde_json::from_slice(&ops2[0].payload).unwrap();
        assert_eq!(
            payload2["source"],
            json!("peer:origin"),
            "first-hop padded source is trimmed in the payload source"
        );
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
            schema_version: None,
            registry_collection: None,
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
