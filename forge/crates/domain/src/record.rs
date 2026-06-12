//! Forward-compatible record envelope.
//!
//! prd-merged/02-data-layer-prd.md §5 (DL-7..DL-12). Every logical record
//! round-trips an envelope that preserves fields/features a client doesn't
//! recognize (DL-9 unknown-field preservation) and carries stable field ids.
//!
//! In M0a the envelope is the unit the `ctx.db` host API reads/writes and the
//! storage projection materializes.

use crate::ids::{CollectionId, RecordId};
use crate::LogicalTimestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current envelope schema version (DL §5 `envelope_version`).
pub const ENVELOPE_VERSION: u32 = 1;

/// A logical record as stored and synced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordEnvelope {
    #[serde(default = "default_envelope_version")]
    pub envelope_version: u32,
    pub entity_id: RecordId,
    pub collection: CollectionId,
    /// Display-named fields (what applet code reads/writes). The
    /// schema-registry maps these to stable field ids; M0a keeps both for
    /// readability of the spine.
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    /// Fields keyed by stable field id (DL-7). Authoritative for merge.
    #[serde(default)]
    pub field_ids: BTreeMap<String, serde_json::Value>,
    /// Fields whose ids this client does not recognize — preserved verbatim
    /// and never stripped on read-modify-write (DL-9). **Normative.**
    #[serde(default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
    /// Free-form forward-compat slot (DL-13 reserves room; unknown features).
    #[serde(default)]
    pub extensions: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub created_at: LogicalTimestamp,
    #[serde(default)]
    pub updated_at: LogicalTimestamp,
    #[serde(default)]
    pub deleted: bool,
}

fn default_envelope_version() -> u32 {
    ENVELOPE_VERSION
}

impl RecordEnvelope {
    /// A fresh record in `collection` with display-named `fields`.
    pub fn new(
        collection: CollectionId,
        entity_id: RecordId,
        fields: BTreeMap<String, serde_json::Value>,
        at: LogicalTimestamp,
    ) -> Self {
        RecordEnvelope {
            envelope_version: ENVELOPE_VERSION,
            entity_id,
            collection,
            fields,
            field_ids: BTreeMap::new(),
            unknown_fields: BTreeMap::new(),
            extensions: BTreeMap::new(),
            created_at: at,
            updated_at: at,
            deleted: false,
        }
    }

    /// Merge `incoming` into `self`, preserving unknown fields (DL-9).
    ///
    /// Known display fields are overwritten by the incoming value (the spine's
    /// LWW stand-in — full per-field CRDT merge is the `crdt` crate's job);
    /// crucially, any `unknown_fields`/`extensions` already present that the
    /// incoming record lacks are retained, never dropped.
    pub fn merge_known(&mut self, incoming: &RecordEnvelope) {
        for (k, v) in &incoming.fields {
            self.fields.insert(k.clone(), v.clone());
        }
        for (k, v) in &incoming.field_ids {
            self.field_ids.insert(k.clone(), v.clone());
        }
        // Preserve-by-union: incoming unknown/extension fields are added, but
        // existing ones are NOT removed just because incoming omitted them.
        for (k, v) in &incoming.unknown_fields {
            self.unknown_fields.insert(k.clone(), v.clone());
        }
        for (k, v) in &incoming.extensions {
            self.extensions.insert(k.clone(), v.clone());
        }
        self.updated_at = incoming.updated_at.max(self.updated_at);
        self.deleted = incoming.deleted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn envelope_roundtrips() {
        let r = RecordEnvelope::new(
            CollectionId::new("tasks"),
            RecordId::new("rec_1"),
            fields(&[("title", serde_json::json!("Ship MVP"))]),
            LogicalTimestamp(3),
        );
        let s = serde_json::to_string(&r).unwrap();
        let back: RecordEnvelope = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn unknown_fields_survive_read_modify_write() {
        // A "v3" record arrives with a field id this client doesn't know.
        let mut stored = RecordEnvelope::new(
            CollectionId::new("tasks"),
            RecordId::new("rec_1"),
            fields(&[("title", serde_json::json!("Ship"))]),
            LogicalTimestamp(1),
        );
        stored
            .unknown_fields
            .insert("f_future".into(), serde_json::json!({"x": 1}));

        // This client edits the title and writes back, with no knowledge of
        // f_future. DL-9: it must NOT be stripped.
        let mut edit = RecordEnvelope::new(
            CollectionId::new("tasks"),
            RecordId::new("rec_1"),
            fields(&[("title", serde_json::json!("Ship MVP"))]),
            LogicalTimestamp(2),
        );
        // edit has no unknown_fields at all.
        assert!(edit.unknown_fields.is_empty());

        stored.merge_known(&edit);
        assert_eq!(stored.fields["title"], serde_json::json!("Ship MVP"));
        assert_eq!(
            stored.unknown_fields["f_future"],
            serde_json::json!({"x": 1}),
            "unknown field must be preserved across edit (DL-9)"
        );
        // Symmetric direction: editing via the edit envelope then merging back.
        edit.merge_known(&stored);
        assert!(edit.unknown_fields.contains_key("f_future"));
    }

    #[test]
    fn missing_envelope_version_defaults() {
        let json = r#"{"entity_id":"r","collection":"c"}"#;
        let r: RecordEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(r.envelope_version, ENVELOPE_VERSION);
    }
}
