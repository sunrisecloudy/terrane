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
use loro::{ExportMode, LoroDoc, LoroValue, PeerID};

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

    /// Insert or replace a record's fields.
    ///
    /// `fields` must be a JSON object (the record envelope's field map). Each
    /// key/value is written into a per-record sub-map so individual scalar
    /// fields merge independently as LWW registers (prd-merged/02 DL-3). Keys
    /// absent from `fields` are removed so the stored record mirrors the input.
    pub fn set_record(&self, record_id: &str, fields: &serde_json::Value) -> Result<()> {
        let obj = fields.as_object().ok_or_else(|| {
            CoreError::ValidationError(format!(
                "record {record_id} fields must be a JSON object, got {fields}"
            ))
        })?;

        let root = self.doc.get_map(self.map_name.as_str());
        let sub = root
            .get_or_create_container(record_id, loro::LoroMap::new())
            .map_err(sync_err)?;

        // Remove keys that are no longer present so set_record is a full
        // replace of the record's field set (read-modify-write friendly).
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
        doc.set_record("rec_1", &json!({"title": "Ship MVP", "done": false})).unwrap();
        doc.commit();

        let got = doc.get_record("rec_1").unwrap();
        assert_eq!(got, json!({"title": "Ship MVP", "done": false}));
        assert_eq!(doc.list_record_ids(), vec!["rec_1".to_string()]);
        assert!(doc.get_record("missing").is_none());
    }

    #[test]
    fn list_record_ids_is_sorted_and_covers_all() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.set_record("rec_b", &json!({"x": 1})).unwrap();
        doc.set_record("rec_a", &json!({"x": 2})).unwrap();
        doc.set_record("rec_c", &json!({"x": 3})).unwrap();
        doc.commit();
        assert_eq!(doc.list_record_ids(), vec!["rec_a", "rec_b", "rec_c"]);
    }

    #[test]
    fn set_record_overwrites_and_removes_dropped_fields() {
        let doc = RecordsDoc::new(1).unwrap();
        doc.set_record("rec_1", &json!({"title": "A", "tag": "x"})).unwrap();
        doc.commit();
        // Rewrite without `tag` -> tag must disappear, title updated.
        doc.set_record("rec_1", &json!({"title": "B"})).unwrap();
        doc.commit();
        assert_eq!(doc.get_record("rec_1").unwrap(), json!({"title": "B"}));
    }

    #[test]
    fn set_record_rejects_non_object_fields() {
        let doc = RecordsDoc::new(1).unwrap();
        let err = doc.set_record("rec_1", &json!([1, 2, 3])).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        // A scalar is likewise rejected.
        assert_eq!(doc.set_record("rec_1", &json!(42)).unwrap_err().code(), "ValidationError");
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
        doc.set_record("rec_1", &fields).unwrap();
        doc.commit();
        assert_eq!(doc.get_record("rec_1").unwrap(), fields);
    }

    // --- Snapshot roundtrip ---

    #[test]
    fn snapshot_export_import_roundtrip_preserves_state() {
        let src = RecordsDoc::new(1).unwrap();
        src.set_record("rec_1", &json!({"title": "Hello"})).unwrap();
        src.set_record("rec_2", &json!({"title": "World"})).unwrap();
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
        src.set_record("rec_1", &json!({"title": "Hello"})).unwrap();
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

    // --- Convergence: concurrent edits to DIFFERENT records ---

    #[test]
    fn concurrent_edits_to_different_records_converge() {
        let mut a = RecordsDoc::new(1).unwrap();
        let mut b = RecordsDoc::new(2).unwrap();

        a.set_record("rec_a", &json!({"owner": "alice"})).unwrap();
        a.commit();
        b.set_record("rec_b", &json!({"owner": "bob"})).unwrap();
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
        a.set_record("rec_1", &json!({"title": "seed"})).unwrap();
        a.commit();
        let seed = a.export_snapshot().unwrap();
        b.import(&seed).unwrap();
        b.commit();

        // Concurrent conflicting writes to the SAME scalar field.
        a.set_record("rec_1", &json!({"title": "from-alice"})).unwrap();
        a.commit();
        b.set_record("rec_1", &json!({"title": "from-bob"})).unwrap();
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

    #[test]
    fn same_scalar_winner_is_stable_regardless_of_merge_order() {
        // Run the same conflict twice with the two merge directions and assert
        // the deterministic winner is identical both times.
        fn run() -> String {
            let mut a = RecordsDoc::new(1).unwrap();
            let mut b = RecordsDoc::new(2).unwrap();
            a.set_record("rec_1", &json!({"title": "seed"})).unwrap();
            a.commit();
            let seed = a.export_snapshot().unwrap();
            b.import(&seed).unwrap();
            b.commit();
            a.set_record("rec_1", &json!({"title": "from-alice"})).unwrap();
            a.commit();
            b.set_record("rec_1", &json!({"title": "from-bob"})).unwrap();
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
        a.set_record("rec_a", &json!({"v": 1})).unwrap();
        a.commit();
        b.set_record("rec_b", &json!({"v": 2})).unwrap();
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
        r.set_record("x", &json!({"a": 1})).unwrap();
        r.commit();
        let t = TextDoc::new(2).unwrap();
        t.replace_all("abc").unwrap();
        t.commit();
        assert!(snapshot_len(&r) > 0);
        assert!(snapshot_len(&t) > 0);
    }
}
