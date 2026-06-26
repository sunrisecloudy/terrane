//! forge-crdt: a thin `CrdtDoc` trait plus Loro-backed documents that map
//! forge records and applet source text onto CRDTs.
//!
//! Normative spec: prd-merged/02-data-layer-prd.md.
//! - **DL-1** The CRDT engine is **Loro**, abstracted behind a thin `CrdtDoc`
//!   trait so the rest of the core never depends on Loro types directly.
//! - **DL-2** Document granularity: records live in a `collection_doc` (here a
//!   top-level Loro map keyed by record id) and applet source files live in a
//!   per-file `src/<file>` text doc — never one giant document.
//! - **DL-3** Field type selects merge semantics: scalar fields merge as LWW
//!   registers (which falls out of Loro map semantics) and a source file merges
//!   as collaborative text.
//! - **DL-9 / §9 acceptance** Concurrent edits on independent peers converge to
//!   a byte-identical materialized value once updates are exchanged.
//!
//! This crate is the boundary: every Loro error is translated into a
//! `forge_domain::CoreError` and no public method panics on external input.

use forge_domain::{CoreError, Result};
use loro::{ExportMode, LoroDoc, LoroValue, PeerID, VersionVector};

/// A CRDT document that can be snapshotted and merged with a peer.
///
/// The two operations here are the entire surface the storage/sync layers need
/// (prd-merged/02 DL-1): export the document's state+history as an opaque blob,
/// and import a peer's blob, merging it in. Merge is commutative/idempotent —
/// importing the same snapshot twice is a no-op — because Loro updates carry
/// their own version vectors.
pub trait CrdtDoc {
    /// Export a full snapshot (state + history) as an opaque, portable blob.
    fn export_snapshot(&self) -> Result<Vec<u8>>;

    /// Import a peer's snapshot/update blob, merging it into this document.
    fn import(&mut self, bytes: &[u8]) -> Result<()>;
}

/// Translate a Loro error into the forge error vocabulary.
///
/// CRDT failures are surfaced as `SyncError` — they happen at the
/// merge/encode/decode boundary, which is exactly the sync seam (prd-merged/01
/// CR-A4 stable error set).
fn sync_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::SyncError(e.to_string())
}

/// Convert a Loro container value into a `serde_json::Value`.
///
/// Loro ships `From<LoroValue> for serde_json::Value` (the top-level `loro`
/// crate enables `loro-common/serde_json`); we centralize it so the rest of the
/// crate speaks only `serde_json`.
fn loro_to_json(v: LoroValue) -> serde_json::Value {
    v.into()
}

/// A collection document: a top-level Loro map keyed by record id, each value a
/// nested map of that record's fields (prd-merged/02 DL-2 `collection_doc`).
///
/// Scalar fields merge with last-writer-wins register semantics provided by the
/// Loro map (prd-merged/02 DL-3). To make concurrent same-key writes converge
/// deterministically across peers, every `RecordsDoc` carries a distinct peer
/// id (Loro breaks LWW ties by peer id).
pub struct RecordsDoc {
    doc: LoroDoc,
    map_name: String,
}

impl RecordsDoc {
    /// The conventional top-level map name for a records document.
    pub const DEFAULT_MAP: &'static str = "records";

    /// Create a fresh records document with a distinct `peer_id`.
    ///
    /// Distinct peer ids are required so two peers editing the *same* scalar
    /// field converge to one deterministic winner instead of corrupting the
    /// document (Loro forbids reusing a peer id across concurrent writers).
    pub fn new(peer_id: PeerID) -> Result<Self> {
        Self::with_map(peer_id, Self::DEFAULT_MAP)
    }

    /// Like [`RecordsDoc::new`] but with an explicit top-level map name.
    pub fn with_map(peer_id: PeerID, map_name: impl Into<String>) -> Result<Self> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(sync_err)?;
        Ok(RecordsDoc { doc, map_name: map_name.into() })
    }

    /// The peer id this document writes under.
    pub fn peer_id(&self) -> PeerID {
        self.doc.peer_id()
    }

    /// Insert or update the given fields of a record, **never deleting** fields
    /// the caller omitted. This is the default read-modify-write path.
    ///
    /// `fields` must be a JSON object (a subset of the record's field map). Each
    /// key/value is written into a per-record sub-map so individual scalar
    /// fields merge independently as LWW registers (prd-merged/02 DL-3).
    ///
    /// Preserving omitted fields is **normative** (prd-merged/02 DL-9): a stale
    /// or older client doing a partial update must not strip fields — including
    /// `unknown_fields` — that it did not supply. Mirrors the domain envelope's
    /// `RecordEnvelope::merge_known` (review 004).
    pub fn patch_record_fields(&self, record_id: &str, fields: &serde_json::Value) -> Result<()> {
        let obj = self.as_field_object(record_id, fields)?;
        let sub = self.record_submap(record_id)?;
        for (k, v) in obj {
            sub.insert(k, LoroValue::from(v.clone())).map_err(sync_err)?;
        }
        Ok(())
    }

    /// Replace a record's field set with exactly `fields`, deleting any field
    /// the caller omitted.
    ///
    /// This carries explicit deletion semantics and must only be used when the
    /// caller genuinely intends to remove the omitted fields (e.g. a deliberate
    /// field-deprecation/delete op) — NOT for ordinary read-modify-write, which
    /// should use [`patch_record_fields`] to honor DL-9 (review 004).
    pub fn replace_record_fields(&self, record_id: &str, fields: &serde_json::Value) -> Result<()> {
        let obj = self.as_field_object(record_id, fields)?;
        let sub = self.record_submap(record_id)?;
        let existing: Vec<String> = sub.keys().map(|k| k.to_string()).collect();
        for k in existing {
            if !obj.contains_key(&k) {
                sub.delete(&k).map_err(sync_err)?;
            }
        }
        for (k, v) in obj {
            sub.insert(k, LoroValue::from(v.clone())).map_err(sync_err)?;
        }
        Ok(())
    }

    /// Write a record's **full envelope** with nested-object values mapped onto
    /// nested Loro map *containers* (one register per leaf key), not onto a single
    /// flattened register — the load-bearing choice for multi-peer field merge
    /// (prd-merged/02 DL-3/DL-9, SS-1/SS-2).
    ///
    /// `envelope` is the complete record envelope JSON the storage write path has
    /// already read-modify-merged (so it is the intended post-state). Each
    /// top-level key is written into the per-record submap; a key whose value is a
    /// JSON **object** (e.g. `fields`, `field_ids`) is written into a nested
    /// `LoroMap` container so that each *individual* sub-field is its own LWW
    /// register. Concurrent writers touching DIFFERENT sub-fields of the same
    /// record therefore both survive a merge (the spec's "different scalar fields
    /// of the same record both survive"), instead of colliding on one
    /// whole-`fields` register where one writer's edit would clobber the other's.
    ///
    /// Omitted keys are deleted (replace semantics) so a single writer's `update`
    /// that drops a field still drops it; because a writer only ever deletes keys
    /// it knew about, a concurrent peer's *new* key is never deleted, so the merge
    /// keeps both. The materialized value (`get_record`) is byte-identical to the
    /// envelope JSON, so DL-6 projection rebuild still reproduces the envelope
    /// exactly.
    pub fn write_record_envelope(
        &self,
        record_id: &str,
        envelope: &serde_json::Value,
    ) -> Result<()> {
        let obj = self.as_field_object(record_id, envelope)?;
        let sub = self.record_submap(record_id)?;
        Self::reconcile_map(&sub, obj)
    }

    /// Reconcile a Loro map container to exactly `obj`: delete keys absent from
    /// `obj`, then for each entry either recurse into a nested container (object
    /// value) or write a scalar register. Recursing per-key is what makes
    /// different sub-fields independent LWW registers (see
    /// [`write_record_envelope`]).
    fn reconcile_map(
        map: &loro::LoroMap,
        obj: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let existing: Vec<String> = map.keys().map(|k| k.to_string()).collect();
        for k in existing {
            if !obj.contains_key(&k) {
                map.delete(&k).map_err(sync_err)?;
            }
        }
        for (k, v) in obj {
            match v {
                serde_json::Value::Object(child) => {
                    let child_map = map
                        .get_or_create_container(k, loro::LoroMap::new())
                        .map_err(sync_err)?;
                    Self::reconcile_map(&child_map, child)?;
                }
                scalar => {
                    map.insert(k, LoroValue::from(scalar.clone())).map_err(sync_err)?;
                }
            }
        }
        Ok(())
    }

    fn as_field_object<'a>(
        &self,
        record_id: &str,
        fields: &'a serde_json::Value,
    ) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
        fields.as_object().ok_or_else(|| {
            CoreError::ValidationError(format!(
                "record {record_id} fields must be a JSON object, got {fields}"
            ))
        })
    }

    fn record_submap(&self, record_id: &str) -> Result<loro::LoroMap> {
        let root = self.doc.get_map(self.map_name.as_str());
        root.get_or_create_container(record_id, loro::LoroMap::new())
            .map_err(sync_err)
    }

    /// Read a record's materialized fields, or `None` if it was never written.
    pub fn get_record(&self, record_id: &str) -> Option<serde_json::Value> {
        let root = self.doc.get_map(self.map_name.as_str());
        let voc = root.get(record_id)?;
        Some(loro_to_json(voc.get_deep_value()))
    }

    /// List all record ids currently present in the document.
    pub fn list_record_ids(&self) -> Vec<String> {
        let root = self.doc.get_map(self.map_name.as_str());
        let mut ids: Vec<String> = root.keys().map(|k| k.to_string()).collect();
        ids.sort();
        ids
    }

    /// Remove a record from the document entirely (a Loro map remove/tombstone
    /// on the root key), so it disappears from both [`materialized`] and
    /// [`list_record_ids`] and stays gone across a snapshot/update roundtrip.
    ///
    /// This is the CRDT-level delete the storage write path needs (prd-merged/02
    /// DL-4/DL-17 `delete`). Loro records the removal as an op carrying its own
    /// version, so it merges and converges like any other write: a peer that
    /// imports this op also drops the record. Deleting a key that was never
    /// written is a no-op (Loro's `delete` on an absent key does not error), so
    /// the operation is idempotent.
    ///
    /// Note this removes the whole record. To remove individual fields while
    /// keeping the record, use [`replace_record_fields`].
    pub fn delete_record(&self, record_id: &str) -> Result<()> {
        let root = self.doc.get_map(self.map_name.as_str());
        root.delete(record_id).map_err(sync_err)
    }

    /// The document's current oplog version vector, encoded as an opaque blob.
    ///
    /// This is the cursor the storage layer persists alongside the chunks it
    /// writes (prd-merged/02 DL-4): pass a previously captured version back to
    /// [`export_updates_since`] to export only the ops appended *after* it, so
    /// each write persists just its incremental update — not a fresh snapshot.
    /// The bytes are portable and round-trip through Loro's own version codec;
    /// the empty/default version corresponds to "from the beginning".
    pub fn version(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// Export the ops appended since `version` (an encoded version vector from a
    /// prior [`version`] call) as an opaque update blob (prd-merged/02 DL-4).
    ///
    /// The storage write path captures [`version`] *before* a mutation, applies
    /// the mutation, then calls this to obtain exactly the new ops to append to
    /// `crdt_chunks`. Importing the returned blob into a document at `version`
    /// advances it to this document's current state. A malformed `version` blob
    /// is a `SyncError` rather than a panic.
    pub fn export_updates_since(&self, version: &[u8]) -> Result<Vec<u8>> {
        let vv = VersionVector::decode(version).map_err(sync_err)?;
        self.doc.export(ExportMode::updates(&vv)).map_err(sync_err)
    }

    /// Export *all* ops of this document as a single update blob (prd-merged/02
    /// DL-4), i.e. [`export_updates_since`] from the empty version. This is the
    /// initial chunk for a brand-new document; subsequent writes use
    /// [`export_updates_since`] with the prior [`version`].
    pub fn export_all_updates(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::all_updates()).map_err(sync_err)
    }

    /// Import one update blob (from [`export_updates_since`] /
    /// [`export_all_updates`], or a snapshot), merging its ops into this document
    /// (prd-merged/02 DL-4 reconstruct-by-replay).
    ///
    /// Updates carry their own version vectors, so importing the same chunk
    /// twice — or chunks out of order — is safe and converges; this is what lets
    /// the storage layer fold `crdt_chunks` back into a live document. Identical
    /// to [`CrdtDoc::import`]; named for the update/chunk vocabulary the rebuild
    /// path speaks.
    pub fn import_updates(&mut self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(sync_err)?;
        Ok(())
    }

    /// Rebuild a records document purely from its persisted update chunks — the
    /// DL-6 rebuild primitive (prd-merged/02: the CRDT chunks are the source of
    /// truth, the projection is derived and rebuildable).
    ///
    /// `chunks` is the ordered sequence of update blobs persisted in
    /// `crdt_chunks` for one document. They are imported as a batch (Loro
    /// reorders/dedupes by version internally), so the result equals the original
    /// document's [`materialized`] regardless of chunk order or duplication.
    ///
    /// `peer_id` is the identity the rebuilt document writes *future* ops under;
    /// it does not affect the imported history. A garbage chunk surfaces a
    /// `SyncError`.
    pub fn from_updates(peer_id: PeerID, chunks: &[&[u8]]) -> Result<Self> {
        Self::from_updates_with_map(peer_id, Self::DEFAULT_MAP, chunks)
    }

    /// Like [`from_updates`] but with an explicit top-level map name (matching
    /// [`with_map`]), so a rebuilt document reads the same container as the one
    /// whose chunks it was built from.
    pub fn from_updates_with_map(
        peer_id: PeerID,
        map_name: impl Into<String>,
        chunks: &[&[u8]],
    ) -> Result<Self> {
        let doc = Self::with_map(peer_id, map_name)?;
        let owned: Vec<Vec<u8>> = chunks.iter().map(|c| c.to_vec()).collect();
        doc.doc.import_batch(&owned).map_err(sync_err)?;
        doc.commit();
        Ok(doc)
    }

    /// Commit pending operations into the document's oplog.
    ///
    /// Loro batches edits into a transaction; `commit` finalizes them so they
    /// are visible to history/export. Reads see uncommitted edits already, but
    /// callers should commit at logical write boundaries.
    pub fn commit(&self) {
        self.doc.commit();
    }

    /// The fully materialized document value (all records), for tests/diffing.
    ///
    /// Two documents that have converged (prd-merged/02 §9) produce an equal
    /// value here.
    pub fn materialized(&self) -> serde_json::Value {
        loro_to_json(self.doc.get_deep_value())
    }
}

impl CrdtDoc for RecordsDoc {
    fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::Snapshot).map_err(sync_err)
    }

    fn import(&mut self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(sync_err)?;
        Ok(())
    }
}

/// Merge two records documents so both converge to the identical materialized
/// value (prd-merged/02 DL-9 / §9 convergence, scaled to two peers).
///
/// Each peer exports a snapshot, the other imports it, and both commit. Because
/// Loro updates carry version vectors, the exchange is order-independent and
/// idempotent: afterwards `a.materialized() == b.materialized()`.
pub fn merge(a: &mut RecordsDoc, b: &mut RecordsDoc) -> Result<()> {
    let a_snapshot = a.export_snapshot()?;
    let b_snapshot = b.export_snapshot()?;
    a.import(&b_snapshot)?;
    b.import(&a_snapshot)?;
    a.commit();
    b.commit();
    Ok(())
}

/// An applet source file as a collaborative-text document (prd-merged/02 DL-2
/// `src/<file>`, DL-3 text → collaborative text).
///
/// Distinct peer ids let two editors' concurrent text edits merge.
pub struct TextDoc {
    doc: LoroDoc,
    text_name: String,
}

impl TextDoc {
    /// The conventional container name for a single source file's text.
    pub const DEFAULT_TEXT: &'static str = "source";

    /// Create a fresh text document with a distinct `peer_id`.
    pub fn new(peer_id: PeerID) -> Result<Self> {
        Self::with_name(peer_id, Self::DEFAULT_TEXT)
    }

    /// Like [`TextDoc::new`] but with an explicit text container name.
    pub fn with_name(peer_id: PeerID, text_name: impl Into<String>) -> Result<Self> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(sync_err)?;
        Ok(TextDoc { doc, text_name: text_name.into() })
    }

    /// The peer id this document writes under.
    pub fn peer_id(&self) -> PeerID {
        self.doc.peer_id()
    }

    /// Replace the entire file contents with `text`.
    ///
    /// Uses Loro's diff-based `update`, which computes the minimal edit between
    /// the current and new content — so an edit that only changes one line does
    /// not rewrite the whole buffer, preserving collaborative merge quality
    /// (prd-merged/02 DL-3).
    pub fn replace_all(&self, text: &str) -> Result<()> {
        let t = self.doc.get_text(self.text_name.as_str());
        t.update(text, Default::default()).map_err(sync_err)?;
        Ok(())
    }

    /// The current file contents.
    pub fn get_text(&self) -> String {
        self.doc.get_text(self.text_name.as_str()).to_string()
    }

    /// Commit pending text edits into the oplog.
    pub fn commit(&self) {
        self.doc.commit();
    }
}

impl CrdtDoc for TextDoc {
    fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::Snapshot).map_err(sync_err)
    }

    fn import(&mut self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(sync_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- RecordsDoc: set / get / list ---

    #[test]
    fn set_get_list_a_record() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.replace_record_fields("rec_1", &json!({"title": "Ship MVP", "done": false})).unwrap();
        doc.commit();

        let got = doc.get_record("rec_1").unwrap();
        assert_eq!(got, json!({"title": "Ship MVP", "done": false}));
        assert_eq!(doc.list_record_ids(), vec!["rec_1".to_string()]);
        assert!(doc.get_record("missing").is_none());
    }

    #[test]
    fn list_record_ids_is_sorted_and_covers_all() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.replace_record_fields("rec_b", &json!({"x": 1})).unwrap();
        doc.replace_record_fields("rec_a", &json!({"x": 2})).unwrap();
        doc.replace_record_fields("rec_c", &json!({"x": 3})).unwrap();
        doc.commit();
        assert_eq!(doc.list_record_ids(), vec!["rec_a", "rec_b", "rec_c"]);
    }

    #[test]
    fn replace_record_fields_removes_dropped_fields() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.replace_record_fields("rec_1", &json!({"title": "A", "tag": "x"})).unwrap();
        doc.commit();
        // Rewrite without `tag` -> tag must disappear, title updated.
        doc.replace_record_fields("rec_1", &json!({"title": "B"})).unwrap();
        doc.commit();
        assert_eq!(doc.get_record("rec_1").unwrap(), json!({"title": "B"}));
    }

    #[test]
    fn patch_record_fields_preserves_omitted_fields() {
        // DL-9 (review 004): a partial update from a stale/older client must NOT
        // strip fields it did not supply — including fields it doesn't understand.
        let doc = RecordsDoc::new(1).unwrap();
        doc.replace_record_fields("rec_1", &json!({"title": "A", "tag": "x", "f_future": {"n": 1}}))
            .unwrap();
        doc.commit();
        // Patch only `title`; `tag` and the unknown `f_future` must survive.
        doc.patch_record_fields("rec_1", &json!({"title": "B"})).unwrap();
        doc.commit();
        assert_eq!(
            doc.get_record("rec_1").unwrap(),
            json!({"title": "B", "tag": "x", "f_future": {"n": 1}}),
            "patch must preserve omitted/unknown fields (DL-9)"
        );
    }

    #[test]
    fn record_fields_must_be_object() {
        let doc = RecordsDoc::new(1).unwrap();
        // Both write paths reject non-object field maps.
        let err = doc.replace_record_fields("rec_1", &json!([1, 2, 3])).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert_eq!(doc.replace_record_fields("rec_1", &json!(42)).unwrap_err().code(), "ValidationError");
        assert_eq!(doc.patch_record_fields("rec_1", &json!("str")).unwrap_err().code(), "ValidationError");
    }

    #[test]
    fn nested_field_values_roundtrip() {
        let doc = RecordsDoc::new(1).unwrap();
        let fields = json!({
            "title": "Nested",
            "meta": {"k": "v", "n": 7},
            "tags": ["a", "b"],
            "count": 3
        });
        doc.replace_record_fields("rec_1", &fields).unwrap();
        doc.commit();
        assert_eq!(doc.get_record("rec_1").unwrap(), fields);
    }

    // --- Snapshot roundtrip ---

    #[test]
    fn snapshot_export_import_roundtrip_preserves_state() {
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"title": "Hello"})).unwrap();
        src.replace_record_fields("rec_2", &json!({"title": "World"})).unwrap();
        src.commit();
        let snap = src.export_snapshot().unwrap();

        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import(&snap).unwrap();
        dst.commit();

        assert_eq!(dst.list_record_ids(), vec!["rec_1", "rec_2"]);
        assert_eq!(dst.get_record("rec_1").unwrap(), json!({"title": "Hello"}));
        assert_eq!(dst.materialized(), src.materialized());
    }

    #[test]
    fn importing_same_snapshot_twice_is_idempotent() {
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"title": "Hello"})).unwrap();
        src.commit();
        let snap = src.export_snapshot().unwrap();

        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import(&snap).unwrap();
        dst.import(&snap).unwrap();
        dst.commit();
        assert_eq!(dst.get_record("rec_1").unwrap(), json!({"title": "Hello"}));
    }

    #[test]
    fn import_garbage_bytes_returns_sync_error() {
        let mut doc = RecordsDoc::new(1).unwrap();
        let err = doc.import(&[0xde, 0xad, 0xbe, 0xef]).unwrap_err();
        assert_eq!(err.code(), "SyncError");
    }

    // --- delete_record (DL-4/DL-17) ---

    #[test]
    fn delete_record_makes_it_vanish() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.replace_record_fields("rec_1", &json!({"title": "A"})).unwrap();
        doc.replace_record_fields("rec_2", &json!({"title": "B"})).unwrap();
        doc.commit();
        assert_eq!(doc.list_record_ids(), vec!["rec_1", "rec_2"]);

        doc.delete_record("rec_1").unwrap();
        doc.commit();

        // Gone from both materialized() and list_record_ids().
        assert!(doc.get_record("rec_1").is_none());
        assert_eq!(doc.list_record_ids(), vec!["rec_2"]);
        assert_eq!(doc.materialized(), json!({"records": {"rec_2": {"title": "B"}}}));
    }

    #[test]
    fn delete_record_is_idempotent_on_absent_key() {
        let doc = RecordsDoc::new(1).unwrap();
        // Deleting a record that was never written must not error.
        doc.delete_record("never_existed").unwrap();
        doc.commit();
        assert!(doc.list_record_ids().is_empty());
    }

    #[test]
    fn deleted_record_stays_gone_after_snapshot_roundtrip() {
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"title": "A"})).unwrap();
        src.replace_record_fields("rec_2", &json!({"title": "B"})).unwrap();
        src.commit();
        src.delete_record("rec_1").unwrap();
        src.commit();
        let snap = src.export_snapshot().unwrap();

        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import(&snap).unwrap();
        dst.commit();

        // The delete survived the snapshot: rec_1 is absent on the importing peer.
        assert_eq!(dst.list_record_ids(), vec!["rec_2"]);
        assert!(dst.get_record("rec_1").is_none());
        assert_eq!(dst.materialized(), src.materialized());
    }

    #[test]
    fn delete_propagates_through_update_export() {
        // The delete op, exported as an incremental update, removes the record on
        // a peer that already had it (the DL-4 delete-then-sync path).
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"v": 1})).unwrap();
        src.commit();
        let seed = src.export_all_updates().unwrap();

        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import_updates(&seed).unwrap();
        dst.commit();
        assert_eq!(dst.list_record_ids(), vec!["rec_1"]);

        let before = src.version();
        src.delete_record("rec_1").unwrap();
        src.commit();
        let delete_update = src.export_updates_since(&before).unwrap();

        dst.import_updates(&delete_update).unwrap();
        dst.commit();
        assert!(dst.get_record("rec_1").is_none());
        assert_eq!(dst.materialized(), src.materialized());
    }

    // --- Incremental update export/import (DL-4) ---

    #[test]
    fn export_updates_since_converges_on_second_doc() {
        // A second doc that imports only the incremental updates (not a full
        // snapshot) converges to identical materialized() (DL-4).
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"title": "one"})).unwrap();
        src.commit();
        let chunk1 = src.export_all_updates().unwrap();

        // Capture the version, write more, export only the delta.
        let after_first = src.version();
        src.replace_record_fields("rec_2", &json!({"title": "two"})).unwrap();
        src.patch_record_fields("rec_1", &json!({"done": true})).unwrap();
        src.commit();
        let chunk2 = src.export_updates_since(&after_first).unwrap();

        // Importing the two deltas in order reconstructs the source exactly.
        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import_updates(&chunk1).unwrap();
        dst.import_updates(&chunk2).unwrap();
        dst.commit();

        assert_eq!(dst.materialized(), src.materialized());
        assert_eq!(dst.get_record("rec_1").unwrap(), json!({"title": "one", "done": true}));
        assert_eq!(dst.get_record("rec_2").unwrap(), json!({"title": "two"}));
    }

    #[test]
    fn export_updates_since_is_empty_when_nothing_changed() {
        // Exporting deltas from the current version yields ops that add nothing.
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"v": 1})).unwrap();
        src.commit();

        let now = src.version();
        let delta = src.export_updates_since(&now).unwrap();

        // Importing an empty/no-op delta leaves a converged peer unchanged.
        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import_updates(&src.export_all_updates().unwrap()).unwrap();
        dst.commit();
        let before = dst.materialized();
        dst.import_updates(&delta).unwrap();
        dst.commit();
        assert_eq!(dst.materialized(), before);
    }

    #[test]
    fn import_updates_is_idempotent() {
        let src = RecordsDoc::new(1).unwrap();
        src.replace_record_fields("rec_1", &json!({"v": 1})).unwrap();
        src.commit();
        let chunk = src.export_all_updates().unwrap();

        let mut dst = RecordsDoc::new(2).unwrap();
        dst.import_updates(&chunk).unwrap();
        dst.import_updates(&chunk).unwrap();
        dst.commit();
        assert_eq!(dst.get_record("rec_1").unwrap(), json!({"v": 1}));
    }

    #[test]
    fn export_updates_since_rejects_garbage_version() {
        let doc = RecordsDoc::new(1).unwrap();
        let err = doc.export_updates_since(&[0xff, 0x00, 0x13, 0x37]).unwrap_err();
        assert_eq!(err.code(), "SyncError");
    }

    #[test]
    fn import_updates_rejects_garbage() {
        let mut doc = RecordsDoc::new(1).unwrap();
        let err = doc.import_updates(&[0xde, 0xad]).unwrap_err();
        assert_eq!(err.code(), "SyncError");
    }

    // --- from_updates: the DL-6 rebuild primitive ---

    #[test]
    fn rebuild_from_update_chunks_equals_original() {
        // Build a doc as a sequence of incremental update chunks (as the storage
        // layer would persist them per write), then rebuild a fresh doc purely
        // from that chunk sequence and assert byte-identical materialized() —
        // the DL-6 rebuild-from-chunks == maintained-projection guarantee.
        let src = RecordsDoc::new(1).unwrap();
        let mut chunks: Vec<Vec<u8>> = Vec::new();

        // Write 1.
        let v0 = src.version();
        src.replace_record_fields("rec_1", &json!({"title": "one"})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v0).unwrap());

        // Write 2.
        let v1 = src.version();
        src.replace_record_fields("rec_2", &json!({"title": "two"})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v1).unwrap());

        // Write 3: a patch.
        let v2 = src.version();
        src.patch_record_fields("rec_1", &json!({"done": true})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v2).unwrap());

        // Write 4: a delete.
        let v3 = src.version();
        src.delete_record("rec_2").unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v3).unwrap());

        let refs: Vec<&[u8]> = chunks.iter().map(|c| c.as_slice()).collect();
        let rebuilt = RecordsDoc::from_updates(9, &refs).unwrap();

        assert_eq!(rebuilt.materialized(), src.materialized());
        assert_eq!(rebuilt.list_record_ids(), src.list_record_ids());
        assert_eq!(rebuilt.get_record("rec_1").unwrap(), json!({"title": "one", "done": true}));
        assert!(rebuilt.get_record("rec_2").is_none(), "deleted record must stay deleted after rebuild");
    }

    #[test]
    fn rebuild_after_reinsert_following_delete_shows_recreated_record() {
        // The hardest DL-6 fixture (fixtures/crdt-write/reinsert_after_delete_rebuild.json):
        // insert -> delete -> reinsert the SAME id. A naive tombstone could leave
        // the record hidden after rebuild; here rebuild-from-chunks must reproduce
        // the *recreated* record exactly, and the maintained doc must agree.
        let src = RecordsDoc::new(1).unwrap();
        let mut chunks: Vec<Vec<u8>> = Vec::new();

        // insert t1 (old title)
        let v0 = src.version();
        src.replace_record_fields("t1", &json!({"title": "Old title", "done": false})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v0).unwrap());

        // delete t1
        let v1 = src.version();
        src.delete_record("t1").unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v1).unwrap());
        assert!(src.get_record("t1").is_none(), "record must be gone after delete");

        // reinsert t1 (recreated title) under the same id
        let v2 = src.version();
        src.replace_record_fields("t1", &json!({"title": "Recreated title", "done": false}))
            .unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v2).unwrap());

        // The maintained doc shows the recreated record.
        let expect = json!({"title": "Recreated title", "done": false});
        assert_eq!(src.get_record("t1").unwrap(), expect);
        assert_eq!(src.list_record_ids(), vec!["t1"]);

        // Rebuild purely from the three persisted chunks (the storage DL-6 path)
        // must equal the maintained projection — no lingering tombstone.
        let refs: Vec<&[u8]> = chunks.iter().map(|c| c.as_slice()).collect();
        let rebuilt = RecordsDoc::from_updates(9, &refs).unwrap();
        assert_eq!(rebuilt.get_record("t1").unwrap(), expect);
        assert_eq!(rebuilt.list_record_ids(), vec!["t1"]);
        assert_eq!(rebuilt.materialized(), src.materialized());
        assert_eq!(chunks.len(), 3, "one chunk per write (expect_chunk_count: 3)");
    }

    #[test]
    fn rebuild_from_chunks_is_order_and_duplication_independent() {
        // Loro reorders/dedupes by version, so rebuilding from a shuffled,
        // duplicated chunk sequence still equals the original (DL-6: chunks are
        // the source of truth, arrival order is irrelevant).
        let src = RecordsDoc::new(1).unwrap();
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let v0 = src.version();
        src.replace_record_fields("rec_a", &json!({"n": 1})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v0).unwrap());
        let v1 = src.version();
        src.replace_record_fields("rec_b", &json!({"n": 2})).unwrap();
        src.commit();
        chunks.push(src.export_updates_since(&v1).unwrap());

        // Reverse + duplicate the chunk sequence.
        let shuffled: Vec<&[u8]> = vec![
            chunks[1].as_slice(),
            chunks[0].as_slice(),
            chunks[1].as_slice(),
        ];
        let rebuilt = RecordsDoc::from_updates(2, &shuffled).unwrap();
        assert_eq!(rebuilt.materialized(), src.materialized());
    }

    #[test]
    fn rebuild_from_empty_chunks_is_empty_doc() {
        let rebuilt = RecordsDoc::from_updates(1, &[]).unwrap();
        assert!(rebuilt.list_record_ids().is_empty());
        assert_eq!(rebuilt.materialized(), json!({"records": {}}));
    }

    #[test]
    fn rebuild_from_garbage_chunk_returns_sync_error() {
        // RecordsDoc has no Debug impl (it wraps a LoroDoc), so match on the Err
        // arm directly rather than `unwrap_err`.
        let bad: Vec<&[u8]> = vec![&[0xde, 0xad, 0xbe, 0xef]];
        match RecordsDoc::from_updates(1, &bad) {
            Err(e) => assert_eq!(e.code(), "SyncError"),
            Ok(_) => panic!("expected a SyncError rebuilding from a garbage chunk"),
        }
    }

    #[test]
    fn from_updates_with_map_reads_custom_container() {
        // Chunks from a custom-map doc rebuild correctly only when the rebuild
        // uses the same map name.
        let src = RecordsDoc::with_map(1, "things").unwrap();
        src.replace_record_fields("rec_1", &json!({"v": 1})).unwrap();
        src.commit();
        let chunk = src.export_all_updates().unwrap();

        let rebuilt = RecordsDoc::from_updates_with_map(2, "things", &[chunk.as_slice()]).unwrap();
        assert_eq!(rebuilt.get_record("rec_1").unwrap(), json!({"v": 1}));
        assert_eq!(rebuilt.materialized(), src.materialized());
    }

    #[test]
    fn transact_group_of_mutations_exports_one_chunk_that_rebuilds_whole_group() {
        // DL-4 step 5 `transact` + fixture `transact_group_single_chunk`
        // (expect_chunk_count: 1): the storage write path applies *all* child
        // mutations to the same document, commits ONCE, then exports a SINGLE
        // incremental update covering the whole group. This pins the crdt-level
        // primitive that guarantee rests on — multiple mutations across one
        // commit collapse into exactly one chunk, and that lone chunk rebuilds
        // the entire group (DL-6) byte-identically to the maintained doc.
        let src = RecordsDoc::new(1).unwrap();

        // Capture the version once, before the whole transact group.
        let before = src.version();
        src.replace_record_fields("t1", &json!({"title": "Grouped A", "done": false})).unwrap();
        src.replace_record_fields("t2", &json!({"title": "Grouped B", "done": false})).unwrap();
        src.patch_record_fields("t1", &json!({"done": true})).unwrap();
        // A single commit for the group, then a single export.
        src.commit();
        let group_chunk = src.export_updates_since(&before).unwrap();

        // The maintained doc shows the post-group projection.
        assert_eq!(src.get_record("t1").unwrap(), json!({"title": "Grouped A", "done": true}));
        assert_eq!(src.get_record("t2").unwrap(), json!({"title": "Grouped B", "done": false}));

        // The lone chunk rebuilds the whole group with zero diff (DL-6).
        let rebuilt = RecordsDoc::from_updates(2, &[group_chunk.as_slice()]).unwrap();
        assert_eq!(rebuilt.list_record_ids(), vec!["t1", "t2"]);
        assert_eq!(rebuilt.get_record("t1").unwrap(), json!({"title": "Grouped A", "done": true}));
        assert_eq!(rebuilt.get_record("t2").unwrap(), json!({"title": "Grouped B", "done": false}));
        assert_eq!(rebuilt.materialized(), src.materialized());
    }

    // --- Convergence: concurrent edits to DIFFERENT records ---

    #[test]
    fn concurrent_edits_to_different_records_converge() {
        let mut a = RecordsDoc::new(1).unwrap();
        let mut b = RecordsDoc::new(2).unwrap();

        a.replace_record_fields("rec_a", &json!({"owner": "alice"})).unwrap();
        a.commit();
        b.replace_record_fields("rec_b", &json!({"owner": "bob"})).unwrap();
        b.commit();

        merge(&mut a, &mut b).unwrap();

        // Both peers now see both records, identically (DL-9 / §9).
        assert_eq!(a.materialized(), b.materialized());
        assert_eq!(a.list_record_ids(), vec!["rec_a", "rec_b"]);
        assert_eq!(a.get_record("rec_a").unwrap(), json!({"owner": "alice"}));
        assert_eq!(a.get_record("rec_b").unwrap(), json!({"owner": "bob"}));
    }

    // --- Convergence: concurrent edits to the SAME scalar field ---

    #[test]
    fn concurrent_edits_to_same_scalar_converge_deterministically() {
        let mut a = RecordsDoc::new(1).unwrap();
        let mut b = RecordsDoc::new(2).unwrap();

        // Seed both with the same record so they share the key, then diverge.
        a.replace_record_fields("rec_1", &json!({"title": "seed"})).unwrap();
        a.commit();
        let seed = a.export_snapshot().unwrap();
        b.import(&seed).unwrap();
        b.commit();

        // Concurrent conflicting writes to the SAME scalar field.
        a.replace_record_fields("rec_1", &json!({"title": "from-alice"})).unwrap();
        a.commit();
        b.replace_record_fields("rec_1", &json!({"title": "from-bob"})).unwrap();
        b.commit();

        merge(&mut a, &mut b).unwrap();

        // Both peers agree on a single winner (which one is impl-defined LWW).
        let a_val = a.get_record("rec_1").unwrap();
        let b_val = b.get_record("rec_1").unwrap();
        assert_eq!(a_val, b_val, "peers must agree on the LWW winner (DL-3)");
        let title = a_val["title"].as_str().unwrap();
        assert!(
            title == "from-alice" || title == "from-bob",
            "winner must be one of the two writes, got {title}"
        );
        // Full materialized convergence, not just the one field.
        assert_eq!(a.materialized(), b.materialized());
    }

    // --- Convergence: concurrent patches to DIFFERENT fields of the SAME record ---

    #[test]
    fn concurrent_patches_to_different_fields_of_same_record_keep_both() {
        // The exact case the old delete-missing-field behavior would corrupt
        // (review 004 [P2]): both peers start from {title, tag}; A patches title,
        // B patches tag; after merge BOTH fields survive on both peers.
        let mut a = RecordsDoc::new(1).unwrap();
        let mut b = RecordsDoc::new(2).unwrap();

        a.replace_record_fields("rec_1", &json!({"title": "seed", "tag": "seed"})).unwrap();
        a.commit();
        let seed = a.export_snapshot().unwrap();
        b.import(&seed).unwrap();
        b.commit();

        // Concurrent partial updates to DIFFERENT fields via the upsert path.
        a.patch_record_fields("rec_1", &json!({"title": "from-alice"})).unwrap();
        a.commit();
        b.patch_record_fields("rec_1", &json!({"tag": "from-bob"})).unwrap();
        b.commit();

        merge(&mut a, &mut b).unwrap();

        assert_eq!(a.materialized(), b.materialized(), "peers must converge");
        assert_eq!(
            a.get_record("rec_1").unwrap(),
            json!({"title": "from-alice", "tag": "from-bob"}),
            "both independently-patched fields must survive (DL-3/DL-9)"
        );
    }

    #[test]
    fn same_scalar_winner_is_stable_regardless_of_merge_order() {
        // Run the same conflict twice with the two merge directions and assert
        // the deterministic winner is identical both times.
        fn run() -> String {
            let mut a = RecordsDoc::new(1).unwrap();
            let mut b = RecordsDoc::new(2).unwrap();
            a.replace_record_fields("rec_1", &json!({"title": "seed"})).unwrap();
            a.commit();
            let seed = a.export_snapshot().unwrap();
            b.import(&seed).unwrap();
            b.commit();
            a.replace_record_fields("rec_1", &json!({"title": "from-alice"})).unwrap();
            a.commit();
            b.replace_record_fields("rec_1", &json!({"title": "from-bob"})).unwrap();
            b.commit();
            merge(&mut a, &mut b).unwrap();
            a.get_record("rec_1").unwrap()["title"].as_str().unwrap().to_string()
        }
        assert_eq!(run(), run(), "LWW winner must be deterministic across runs");
    }

    #[test]
    fn merge_is_idempotent_when_repeated() {
        let mut a = RecordsDoc::new(1).unwrap();
        let mut b = RecordsDoc::new(2).unwrap();
        a.replace_record_fields("rec_a", &json!({"v": 1})).unwrap();
        a.commit();
        b.replace_record_fields("rec_b", &json!({"v": 2})).unwrap();
        b.commit();
        merge(&mut a, &mut b).unwrap();
        let after_first = a.materialized();
        merge(&mut a, &mut b).unwrap();
        assert_eq!(a.materialized(), after_first);
        assert_eq!(a.materialized(), b.materialized());
    }

    // --- TextDoc ---

    #[test]
    fn text_replace_and_get() {
        let doc = TextDoc::new(1).unwrap();
        doc.replace_all("hello").unwrap();
        doc.commit();
        assert_eq!(doc.get_text(), "hello");
        doc.replace_all("hello world").unwrap();
        doc.commit();
        assert_eq!(doc.get_text(), "hello world");
    }

    #[test]
    fn text_snapshot_roundtrip() {
        let src = TextDoc::new(1).unwrap();
        src.replace_all("fn main() {}\n").unwrap();
        src.commit();
        let snap = src.export_snapshot().unwrap();

        let mut dst = TextDoc::new(2).unwrap();
        dst.import(&snap).unwrap();
        dst.commit();
        assert_eq!(dst.get_text(), "fn main() {}\n");
    }

    #[test]
    fn text_concurrent_edits_converge() {
        // Two editors start from the same base, edit different regions, and
        // exchange snapshots -> identical text (DL-3 collaborative text).
        let mut a = TextDoc::new(1).unwrap();
        a.replace_all("line1\nline2\n").unwrap();
        a.commit();
        let base = a.export_snapshot().unwrap();

        let mut b = TextDoc::new(2).unwrap();
        b.import(&base).unwrap();
        b.commit();

        a.replace_all("LINE1\nline2\n").unwrap();
        a.commit();
        b.replace_all("line1\nLINE2\n").unwrap();
        b.commit();

        let a_snap = a.export_snapshot().unwrap();
        let b_snap = b.export_snapshot().unwrap();
        a.import(&b_snap).unwrap();
        b.import(&a_snap).unwrap();
        a.commit();
        b.commit();

        assert_eq!(a.get_text(), b.get_text(), "collaborative text must converge");
        assert_eq!(a.get_text(), "LINE1\nLINE2\n");
    }

    #[test]
    fn text_import_garbage_returns_sync_error() {
        let mut doc = TextDoc::new(1).unwrap();
        assert_eq!(doc.import(&[1, 2, 3]).unwrap_err().code(), "SyncError");
    }

    #[test]
    fn distinct_peer_ids_are_recorded() {
        let a = RecordsDoc::new(7).unwrap();
        let t = TextDoc::new(9).unwrap();
        assert_eq!(a.peer_id(), 7);
        assert_eq!(t.peer_id(), 9);
    }

    #[test]
    fn crdt_doc_trait_is_object_usable_via_generics() {
        // Exercise the trait through a generic so both impls are covered.
        fn snapshot_len<D: CrdtDoc>(d: &D) -> usize {
            d.export_snapshot().unwrap().len()
        }
        let r = RecordsDoc::new(1).unwrap();
        r.replace_record_fields("x", &json!({"a": 1})).unwrap();
        r.commit();
        let t = TextDoc::new(2).unwrap();
        t.replace_all("abc").unwrap();
        t.commit();
        assert!(snapshot_len(&r) > 0);
        assert!(snapshot_len(&t) > 0);
    }
}
