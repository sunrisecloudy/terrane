//! forge-schema: dynamic schema registry with stable field ids and
//! additive-only evolution.
//!
//! prd-merged/02-data-layer-prd.md §5:
//! - DL-7  stable `field_id`, never reused (**per-actor id ranges: actor-id ⊕
//!   counter**, e.g. `f_<actor>_<seq>`); renames touch only the display name;
//! - DL-8  additive-only schema changes (add collection/field, widen type,
//!   deprecate); destructive ops have no API surface;
//! - DL-9  unknown-field preservation/tolerance (validated by stable id first);
//! - DL-10 unknown-collection tolerance;
//! - DL-11 registry versions are CRDT vectors — two offline actors adding a
//!   first field to the same collection merge to the union *by construction*
//!   because their minted ids are actor-scoped and therefore distinct;
//! - DL-12 constraints warn before they enforce.
//!
//! Registry/collection/field internals are private (review 005 P2): external
//! crates read through accessors and mutate only via [`SchemaChange`], so they
//! cannot bypass the additive-only / id-stability invariants. Deserialized
//! state is re-validated via [`SchemaRegistry::validated`].
//!
//! This crate is pure logic with no I/O: the storage layer persists the
//! registry as the `schema_registry_doc` CRDT (DL-2), but every compatibility
//! rule lives here. It must stay `wasm32-unknown-unknown`-clean (no
//! `std::time`/`std::fs`).

mod collection;
mod field_type;
mod migration;
mod registry;

pub use collection::{CollectionDef, FieldDef};
pub use field_type::FieldType;
pub use migration::{migrate_record, FieldTransform, MigrationDescriptor};
pub use registry::{merge_collection_def, SchemaChange, SchemaRegistry, SchemaWarning};

#[cfg(test)]
mod tests {
    //! Cross-module integration tests that exercise the public surface as a
    //! caller (e.g. the data-layer host) would.
    use super::*;
    use forge_domain::{ActorId, CollectionId, LogicalTimestamp, RecordEnvelope, RecordId};
    use std::collections::BTreeMap;

    #[test]
    fn end_to_end_evolution_stays_forward_compatible() {
        // v1: an `expenses` collection with an int `amount` (minted by alice).
        let mut v1 = SchemaRegistry::new();
        v1.apply_change(SchemaChange::AddCollection { name: "expenses".into() }).unwrap();
        v1.apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: true,
            required: false,
        })
        .unwrap();

        // v2 = v1 evolved additively: widen amount to float, add a deprecated
        // note field, add a new collection.
        let mut v2 = v1.clone();
        v2.apply_change(SchemaChange::WidenField {
            collection: "expenses".into(),
            field_id: "f_alice_0".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
        v2.apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "note".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();
        v2.apply_change(SchemaChange::DeprecateField {
            collection: "expenses".into(),
            field_id: "f_alice_1".into(),
        })
        .unwrap();
        v2.apply_change(SchemaChange::AddCollection { name: "tags".into() }).unwrap();

        // The whole point (DL-8): v2 is a valid forward evolution of v1.
        assert!(v2.validate_compatibility(&v1).is_ok());
        // And v1 is the trivial base.
        assert!(v1.validate_compatibility(&SchemaRegistry::new()).is_ok());
    }

    #[test]
    fn old_client_reads_record_with_unknown_field_and_collection() {
        // An "old" client only knows `expenses.amount`.
        let mut old = SchemaRegistry::new();
        old.apply_change(SchemaChange::AddCollection { name: "expenses".into() }).unwrap();
        old.apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: false,
            required: false,
        })
        .unwrap();

        // A "v3" record carries a field this client never declared (DL-9)...
        let mut fields = BTreeMap::new();
        fields.insert("amount".into(), serde_json::json!(10));
        fields.insert("currency".into(), serde_json::json!("USD"));
        let record = RecordEnvelope::new(
            CollectionId::new("expenses"),
            RecordId::new("e1"),
            fields,
            LogicalTimestamp(1),
        );
        let warnings = old.validate_record("expenses", &record).unwrap();
        assert!(
            warnings.iter().any(|w| w.message.contains("currency")),
            "unknown field must be tolerated, not rejected (DL-9): {warnings:?}"
        );

        // ...and a whole collection this client has no schema for (DL-10).
        let other = RecordEnvelope::new(
            CollectionId::new("budgets"),
            RecordId::new("b1"),
            BTreeMap::new(),
            LogicalTimestamp(1),
        );
        assert!(old.validate_record("budgets", &other).is_ok());
    }
}
