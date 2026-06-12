//! Dynamic schema registry with additive-only evolution.
//!
//! prd-merged/02 DL-7 (stable field ids, never reused), DL-8 (additive-only:
//! add collection/field, widen type, deprecate; destructive ops are *not*
//! exposed), DL-9/DL-10 (unknown-collection/field tolerance), DL-12
//! (constraints warn before they enforce).
//!
//! The registry is the authority on "what the schema is". It is pure logic
//! (no I/O, `wasm32`-clean): the storage layer persists it as the
//! `schema_registry_doc` CRDT (DL-2), but the rules live here.

use crate::collection::CollectionDef;
use crate::field_type::FieldType;
use forge_domain::{CoreError, RecordEnvelope, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single additive schema mutation (DL-8). These are the *only* schema
/// operations exposed; there is deliberately no `RemoveField`/`RemoveCollection`
/// or `NarrowField` variant — destructive intent has no API surface, and any
/// attempt to reach a destructive *state* is caught by
/// [`SchemaRegistry::validate_compatibility`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SchemaChange {
    /// Introduce a new, empty collection.
    AddCollection { name: String },
    /// Add a field to an existing collection; mints a fresh stable id (DL-7).
    /// A `required` field is added in warning mode (DL-12).
    AddField {
        collection: String,
        name: String,
        ty: FieldType,
        #[serde(default)]
        indexed: bool,
        #[serde(default)]
        required: bool,
    },
    /// Widen an existing field's type (DL-8). Rejected if `to` is not a valid
    /// widening of the current type (i.e. a narrowing).
    WidenField { collection: String, field_id: String, to: FieldType },
    /// Hide a field from new writes/UI while retaining its data (DL-8). This is
    /// the spine's stand-in for "delete" = deprecate + retain.
    DeprecateField { collection: String, field_id: String },
    /// Flip a field's `required` constraint from warning mode into enforcement
    /// mode (DL-12). Tightening from warn → enforce is itself an explicit,
    /// non-destructive step (existing readers are unaffected).
    EnforceRequired { collection: String, field_id: String },
}

/// A non-fatal validation finding (DL-12 warning mode, DL-9 unknown tolerance).
///
/// Warnings never block a write; they are surfaced to the caller so a UI can
/// nudge the user before a constraint graduates to enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaWarning {
    /// Stable field id the warning concerns, or `None` for record-level/unknown
    /// findings.
    pub field_id: Option<String>,
    /// Human-readable, machine-greppable message.
    pub message: String,
}

impl SchemaWarning {
    fn field(field_id: impl Into<String>, message: impl Into<String>) -> Self {
        SchemaWarning { field_id: Some(field_id.into()), message: message.into() }
    }
    fn record(message: impl Into<String>) -> Self {
        SchemaWarning { field_id: None, message: message.into() }
    }
}

/// The dynamic schema registry: a set of [`CollectionDef`]s keyed by name.
///
/// Registry versions are CRDT vectors in the full data layer (DL-11); for the
/// M0a spine we model a single linearized registry whose only legal evolution
/// is additive, which is exactly what [`Self::validate_compatibility`] checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaRegistry {
    pub collections: BTreeMap<String, CollectionDef>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        SchemaRegistry::default()
    }

    // ----------------------------------------------------------------- queries

    /// DL-10: callers keep, store, and raw-query collections they have no
    /// schema for. This lets them *detect* the unknown case without rejecting.
    pub fn is_known_collection(&self, collection: &str) -> bool {
        self.collections.contains_key(collection)
    }

    /// DL-9: a `field_id` unknown to this registry must be preserved, not
    /// rejected. Returns true only if `collection` is known *and* contains the
    /// field.
    pub fn is_known_field(&self, collection: &str, field_id: &str) -> bool {
        self.collections
            .get(collection)
            .map(|c| c.field(field_id).is_some())
            .unwrap_or(false)
    }

    /// Borrow a collection definition by name.
    pub fn collection(&self, name: &str) -> Option<&CollectionDef> {
        self.collections.get(name)
    }

    // ------------------------------------------------------------------ mutate

    /// Apply an additive [`SchemaChange`], or fail with
    /// [`CoreError::SchemaCompatibilityError`] if it would be destructive or
    /// otherwise invalid (DL-8).
    pub fn apply_change(&mut self, change: SchemaChange) -> Result<()> {
        match change {
            SchemaChange::AddCollection { name } => self.add_collection(&name),
            SchemaChange::AddField { collection, name, ty, indexed, required } => {
                self.add_field(&collection, &name, ty, indexed, required)
            }
            SchemaChange::WidenField { collection, field_id, to } => {
                self.widen_field(&collection, &field_id, to)
            }
            SchemaChange::DeprecateField { collection, field_id } => {
                self.deprecate_field(&collection, &field_id)
            }
            SchemaChange::EnforceRequired { collection, field_id } => {
                self.enforce_required(&collection, &field_id)
            }
        }
    }

    fn add_collection(&mut self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::ValidationError("collection name is empty".into()));
        }
        if self.collections.contains_key(name) {
            // Re-adding an existing collection is rejected (would clobber/reset
            // its field sequence and risk id reuse — DL-7/DL-8).
            return Err(CoreError::SchemaCompatibilityError(format!(
                "collection {name:?} already exists; re-adding is not additive"
            )));
        }
        self.collections.insert(name.to_string(), CollectionDef::new(name));
        Ok(())
    }

    fn add_field(
        &mut self,
        collection: &str,
        name: &str,
        ty: FieldType,
        indexed: bool,
        required: bool,
    ) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::ValidationError("field name is empty".into()));
        }
        let col = self.require_collection_mut(collection)?;
        if col.has_field_name(name) {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "field name {name:?} already exists in collection {collection:?}"
            )));
        }
        col.add_field(name, ty, indexed, required);
        Ok(())
    }

    fn widen_field(&mut self, collection: &str, field_id: &str, to: FieldType) -> Result<()> {
        let col = self.require_collection_mut(collection)?;
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        if field.ty == to {
            return Ok(()); // idempotent no-op widen.
        }
        if !field.ty.can_widen_to(&to) {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "cannot widen field {field_id:?} from {:?} to {to:?}: not an additive widening",
                field.ty
            )));
        }
        field.ty = to;
        Ok(())
    }

    fn deprecate_field(&mut self, collection: &str, field_id: &str) -> Result<()> {
        let col = self.require_collection_mut(collection)?;
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        // Idempotent: deprecating an already-deprecated field is fine.
        field.deprecated = true;
        Ok(())
    }

    fn enforce_required(&mut self, collection: &str, field_id: &str) -> Result<()> {
        let col = self.require_collection_mut(collection)?;
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        if !field.required {
            return Err(CoreError::ValidationError(format!(
                "field {field_id:?} is not a required field; nothing to enforce"
            )));
        }
        field.enforced = true;
        Ok(())
    }

    fn require_collection_mut(&mut self, collection: &str) -> Result<&mut CollectionDef> {
        self.collections.get_mut(collection).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!("unknown collection {collection:?}"))
        })
    }

    // -------------------------------------------------------------- validation

    /// Validate a record against `collection`'s schema (DL-12 validate-on-write).
    ///
    /// Returns `Ok(warnings)` when the record is *acceptable*: only **enforced**
    /// constraints (`enforced && required`) hard-fail with
    /// [`CoreError::ValidationError`]. Everything else — a missing
    /// warn-mode-required value (DL-12), or an unknown field/collection (DL-9/10)
    /// — is reported as a non-fatal [`SchemaWarning`], never an error.
    ///
    /// An unknown collection is *tolerated* (DL-10): the record passes with a
    /// single warning rather than being rejected.
    pub fn validate_record(
        &self,
        collection: &str,
        record: &RecordEnvelope,
    ) -> Result<Vec<SchemaWarning>> {
        let Some(col) = self.collections.get(collection) else {
            // DL-10: tolerate collections we have no schema for.
            return Ok(vec![SchemaWarning::record(format!(
                "unknown collection {collection:?}; record kept untyped (DL-10)"
            ))]);
        };

        let mut warnings = Vec::new();

        // Check declared field constraints against the record's display fields.
        for field in &col.fields {
            if field.deprecated {
                continue; // deprecated fields impose no write constraints (DL-8).
            }
            let present = record.fields.contains_key(&field.name);
            if !present && field.required {
                if field.enforced {
                    // DL-12 enforcement mode: this is now a hard error.
                    return Err(CoreError::ValidationError(format!(
                        "required field {:?} (id {}) missing in collection {collection:?}",
                        field.name, field.field_id
                    )));
                } else {
                    // DL-12 warning mode: warn but accept.
                    warnings.push(SchemaWarning::field(
                        field.field_id.clone(),
                        format!(
                            "field {:?} is required (warn mode) but missing; will become an \
                             error once enforced",
                            field.name
                        ),
                    ));
                }
            }
        }

        // DL-9: any display field the schema does not know is preserved, with a
        // capability-style warning rather than a rejection.
        for name in record.fields.keys() {
            if !col.has_field_name(name) {
                warnings.push(SchemaWarning::record(format!(
                    "field {name:?} is unknown in collection {collection:?}; preserved (DL-9)"
                )));
            }
        }

        Ok(warnings)
    }

    /// Prove that `self` is a forward-compatible, **additive-only** evolution of
    /// `old` (prd-merged/02 DL-8). Returns
    /// [`CoreError::SchemaCompatibilityError`] on any destructive divergence:
    ///
    /// - a collection present in `old` was dropped;
    /// - a field present in `old` was dropped (removal has no API, but this
    ///   catches a hand-built/diverged registry — the test for "removing a field
    ///   is impossible" exercises exactly this guard);
    /// - a field id was **reused** for a different field (DL-7): same id, but the
    ///   old field is no longer a prefix-compatible ancestor;
    /// - a field's type was **narrowed** (old type can no longer widen to new);
    /// - a previously non-deprecated field became... still fine; but an
    ///   *un-deprecation* (deprecated → live) is disallowed as it changes the
    ///   constraint surface retroactively;
    /// - `next_field_seq` went **backwards**, which would risk minting a
    ///   previously-used id (DL-7).
    pub fn validate_compatibility(&self, old: &SchemaRegistry) -> Result<()> {
        for (name, old_col) in &old.collections {
            let new_col = self.collections.get(name).ok_or_else(|| {
                CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?} was removed; schema evolution must be additive (DL-8)"
                ))
            })?;
            Self::check_collection_compatibility(name, old_col, new_col)?;
        }
        // New collections in `self` that are absent from `old` are fine —
        // adding a collection is the canonical additive change (DL-8).
        Ok(())
    }

    fn check_collection_compatibility(
        name: &str,
        old_col: &CollectionDef,
        new_col: &CollectionDef,
    ) -> Result<()> {
        // DL-7: the id sequence must never go backwards (would reuse ids).
        if new_col.next_field_seq < old_col.next_field_seq {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "collection {name:?}: next_field_seq went backwards \
                 ({} -> {}); field ids would be reused (DL-7)",
                old_col.next_field_seq, new_col.next_field_seq
            )));
        }

        for old_field in &old_col.fields {
            let new_field = new_col.field(&old_field.field_id).ok_or_else(|| {
                CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field id {:?} was removed; removal is not additive \
                     (use deprecate; DL-8)",
                    old_field.field_id
                ))
            })?;

            // DL-8: the type may only have widened (or stayed identical).
            if !old_field.ty.can_widen_to(&new_field.ty) {
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field {} type narrowed from {:?} to {:?} (DL-8)",
                    old_field.field_id, old_field.ty, new_field.ty
                )));
            }

            // DL-8: deprecate is one-way (hide + retain). Un-deprecating would
            // resurrect a hidden field and is not an additive change.
            if old_field.deprecated && !new_field.deprecated {
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field {} was un-deprecated; deprecation is one-way \
                     (DL-8)",
                    old_field.field_id
                )));
            }
        }
        // New field ids in `new_col` not present in `old_col` are additive — OK.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{CollectionId, LogicalTimestamp, RecordId};
    use std::collections::BTreeMap as Map;

    fn rec(
        collection: &str,
        fields: &[(&str, serde_json::Value)],
    ) -> RecordEnvelope {
        let map: Map<String, serde_json::Value> =
            fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new("rec_1"),
            map,
            LogicalTimestamp(1),
        )
    }

    /// Helper: a registry with a `tasks` collection holding `title` (text) and
    /// `done` (bool) at ids f0, f1.
    fn tasks_registry() -> SchemaRegistry {
        let mut r = SchemaRegistry::new();
        r.apply_change(SchemaChange::AddCollection { name: "tasks".into() }).unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "title".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "done".into(),
            ty: FieldType::Bool,
            indexed: false,
            required: false,
        })
        .unwrap();
        r
    }

    // -------- DL-7: stable, never-reused field ids --------

    #[test]
    fn add_collection_then_fields_allocate_stable_ids() {
        let r = tasks_registry();
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.fields[0].field_id, "f0");
        assert_eq!(col.fields[1].field_id, "f1");
        assert_eq!(col.next_field_seq, 2);
    }

    #[test]
    fn deprecating_then_adding_does_not_reuse_ids() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f0".into(),
        })
        .unwrap();
        // New field gets f2, NOT the freed-looking f0 (DL-7).
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "title_v2".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.field("f2").unwrap().name, "title_v2");
        assert!(col.field("f0").unwrap().deprecated, "old id still present, hidden");
    }

    #[test]
    fn renaming_a_field_keeps_its_id() {
        // A rename is modelled as: edit the display name in place. We simulate
        // the registry-level rename and assert the id is stable (DL-7).
        let mut r = tasks_registry();
        {
            let col = r.collections.get_mut("tasks").unwrap();
            col.field_mut("f0").unwrap().name = "label".into();
        }
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.field("f0").unwrap().name, "label");
        assert_eq!(col.field("f0").unwrap().field_id, "f0", "rename must not touch id");
        // And it is forward-compatible: a rename is purely additive.
        let old = tasks_registry();
        assert!(r.validate_compatibility(&old).is_ok());
    }

    // -------- DL-8: additive-only evolution --------

    #[test]
    fn re_adding_existing_collection_is_rejected() {
        let mut r = tasks_registry();
        let err = r
            .apply_change(SchemaChange::AddCollection { name: "tasks".into() })
            .unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn duplicate_field_name_is_rejected() {
        let mut r = tasks_registry();
        let err = r
            .apply_change(SchemaChange::AddField {
                collection: "tasks".into(),
                name: "title".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn widen_int_to_float_ok_float_to_int_rejected() {
        let mut r = SchemaRegistry::new();
        r.apply_change(SchemaChange::AddCollection { name: "m".into() }).unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "m".into(),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: false,
            required: false,
        })
        .unwrap();
        // Int -> Float OK.
        r.apply_change(SchemaChange::WidenField {
            collection: "m".into(),
            field_id: "f0".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
        assert_eq!(r.collection("m").unwrap().field("f0").unwrap().ty, FieldType::FloatNum);
        // Float -> Int rejected.
        let err = r
            .apply_change(SchemaChange::WidenField {
                collection: "m".into(),
                field_id: "f0".into(),
                to: FieldType::IntNum,
            })
            .unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn widen_to_same_type_is_idempotent_noop() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::WidenField {
            collection: "tasks".into(),
            field_id: "f0".into(),
            to: FieldType::Text,
        })
        .unwrap();
        assert_eq!(r.collection("tasks").unwrap().field("f0").unwrap().ty, FieldType::Text);
    }

    #[test]
    fn deprecate_hides_but_field_still_listed() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f1".into(),
        })
        .unwrap();
        let col = r.collection("tasks").unwrap();
        assert!(col.field("f1").unwrap().deprecated);
        // Still listed (retained), not removed (DL-8 "delete = deprecate + retain").
        assert_eq!(col.fields.len(), 2);
    }

    #[test]
    fn widen_unknown_field_or_collection_errors() {
        let mut r = tasks_registry();
        assert_eq!(
            r.apply_change(SchemaChange::WidenField {
                collection: "tasks".into(),
                field_id: "f99".into(),
                to: FieldType::Scalar,
            })
            .unwrap_err()
            .code(),
            "SchemaCompatibilityError"
        );
        assert_eq!(
            r.apply_change(SchemaChange::AddField {
                collection: "ghost".into(),
                name: "x".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap_err()
            .code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn empty_names_are_validation_errors() {
        let mut r = SchemaRegistry::new();
        assert_eq!(
            r.apply_change(SchemaChange::AddCollection { name: "  ".into() })
                .unwrap_err()
                .code(),
            "ValidationError"
        );
        r.apply_change(SchemaChange::AddCollection { name: "c".into() }).unwrap();
        assert_eq!(
            r.apply_change(SchemaChange::AddField {
                collection: "c".into(),
                name: "".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap_err()
            .code(),
            "ValidationError"
        );
    }

    // -------- destructive intent rejected via validate_compatibility --------

    #[test]
    fn removing_a_field_is_rejected_by_compatibility() {
        let old = tasks_registry();
        // There is no remove API. Simulate a diverged registry that dropped f1,
        // and prove validate_compatibility catches it (the "removal is
        // impossible" guarantee, enforced structurally).
        let mut diverged = old.clone();
        {
            let col = diverged.collections.get_mut("tasks").unwrap();
            col.fields.retain(|f| f.field_id != "f1");
        }
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        assert!(err.to_string().contains("f1"));
    }

    #[test]
    fn removing_a_collection_is_rejected_by_compatibility() {
        let old = tasks_registry();
        let mut diverged = old.clone();
        diverged.collections.remove("tasks");
        assert_eq!(
            diverged.validate_compatibility(&old).unwrap_err().code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn narrowing_a_field_is_rejected_by_compatibility() {
        let mut old = SchemaRegistry::new();
        old.apply_change(SchemaChange::AddCollection { name: "m".into() }).unwrap();
        old.apply_change(SchemaChange::AddField {
            collection: "m".into(),
            name: "amount".into(),
            ty: FieldType::FloatNum,
            indexed: false,
            required: false,
        })
        .unwrap();
        // Diverged: narrowed Float -> Int.
        let mut diverged = old.clone();
        diverged.collections.get_mut("m").unwrap().field_mut("f0").unwrap().ty =
            FieldType::IntNum;
        assert_eq!(
            diverged.validate_compatibility(&old).unwrap_err().code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn reusing_a_field_id_is_rejected_by_compatibility() {
        let old = tasks_registry();
        // Diverged: f1 ("done", Bool) re-typed to an incompatible type while
        // keeping the same id — a reuse of the id for a different field (DL-7).
        let mut diverged = old.clone();
        {
            let f = diverged.collections.get_mut("tasks").unwrap().field_mut("f1").unwrap();
            f.name = "priority".into();
            f.ty = FieldType::Text; // Bool cannot widen to Text -> reuse detected.
        }
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn seq_going_backwards_is_rejected_by_compatibility() {
        let old = tasks_registry();
        let mut diverged = old.clone();
        diverged.collections.get_mut("tasks").unwrap().next_field_seq = 1;
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        assert!(err.to_string().contains("reused"));
    }

    #[test]
    fn un_deprecating_a_field_is_rejected_by_compatibility() {
        let mut old = tasks_registry();
        old.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f0".into(),
        })
        .unwrap();
        let mut diverged = old.clone();
        diverged.collections.get_mut("tasks").unwrap().field_mut("f0").unwrap().deprecated =
            false;
        assert_eq!(
            diverged.validate_compatibility(&old).unwrap_err().code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn additive_evolution_is_compatible() {
        let old = tasks_registry();
        let mut new = old.clone();
        // Add a field, widen another (after introducing a numeric one), add a
        // new collection, deprecate a field — all additive.
        new.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "priority".into(),
            ty: FieldType::IntNum,
            indexed: true,
            required: false,
        })
        .unwrap();
        new.apply_change(SchemaChange::WidenField {
            collection: "tasks".into(),
            field_id: "f2".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
        new.apply_change(SchemaChange::AddCollection { name: "notes".into() }).unwrap();
        new.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f1".into(),
        })
        .unwrap();
        assert!(new.validate_compatibility(&old).is_ok());
    }

    #[test]
    fn empty_registry_is_compatible_ancestor_of_anything() {
        let old = SchemaRegistry::new();
        let new = tasks_registry();
        assert!(new.validate_compatibility(&old).is_ok());
    }

    // -------- DL-9/DL-10: unknown tolerance --------

    #[test]
    fn known_queries_distinguish_unknowns() {
        let r = tasks_registry();
        assert!(r.is_known_collection("tasks"));
        assert!(!r.is_known_collection("ghost"));
        assert!(r.is_known_field("tasks", "f0"));
        assert!(!r.is_known_field("tasks", "f99"));
        assert!(!r.is_known_field("ghost", "f0"));
    }

    #[test]
    fn unknown_collection_record_is_tolerated_with_warning() {
        let r = tasks_registry();
        let warnings = r.validate_record("ghost", &rec("ghost", &[("x", serde_json::json!(1))]));
        let warnings = warnings.expect("unknown collection must not error (DL-10)");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown collection"));
    }

    #[test]
    fn unknown_field_on_known_collection_is_preserved_with_warning() {
        let r = tasks_registry();
        let record = rec(
            "tasks",
            &[("title", serde_json::json!("hi")), ("mystery", serde_json::json!(7))],
        );
        let warnings = r.validate_record("tasks", &record).unwrap();
        assert!(warnings.iter().any(|w| w.message.contains("mystery")
            && w.message.contains("unknown")));
    }

    // -------- DL-12: warn-before-enforce --------

    #[test]
    fn required_constraint_starts_warn_then_enforces() {
        let mut r = tasks_registry();
        // Add a required field (starts in warn mode per DL-12).
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        let owner_id = r.collection("tasks").unwrap().field("f2").unwrap().field_id.clone();
        assert_eq!(owner_id, "f2");
        assert!(r.collection("tasks").unwrap().field("f2").unwrap().required);
        assert!(!r.collection("tasks").unwrap().field("f2").unwrap().enforced);

        // Record missing `owner`: warn mode => Ok with a warning, NOT an error.
        let record = rec("tasks", &[("title", serde_json::json!("hi"))]);
        let warnings = r.validate_record("tasks", &record).unwrap();
        assert!(
            warnings.iter().any(|w| w.field_id.as_deref() == Some("f2")
                && w.message.contains("warn mode")),
            "warn-mode required must surface a warning, got {warnings:?}"
        );

        // Graduate to enforcement.
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f2".into(),
        })
        .unwrap();
        assert!(r.collection("tasks").unwrap().field("f2").unwrap().enforced);

        // Now the same missing-value record HARD-fails.
        let err = r.validate_record("tasks", &record).unwrap_err();
        assert_eq!(err.code(), "ValidationError");

        // ...but a record that supplies `owner` passes cleanly.
        let ok_record =
            rec("tasks", &[("title", serde_json::json!("hi")), ("owner", serde_json::json!("me"))]);
        assert!(r.validate_record("tasks", &ok_record).unwrap().is_empty());
    }

    #[test]
    fn enforce_on_non_required_field_is_validation_error() {
        let mut r = tasks_registry();
        // f0 (title) is not required.
        let err = r
            .apply_change(SchemaChange::EnforceRequired {
                collection: "tasks".into(),
                field_id: "f0".into(),
            })
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn enforced_required_but_deprecated_field_does_not_block() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f2".into(),
        })
        .unwrap();
        // Then deprecate it: a deprecated field imposes no write constraint.
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f2".into(),
        })
        .unwrap();
        let record = rec("tasks", &[("title", serde_json::json!("hi"))]);
        assert!(
            r.validate_record("tasks", &record).is_ok(),
            "deprecated required field must not hard-block writes"
        );
    }

    #[test]
    fn change_roundtrips_json() {
        let c = SchemaChange::AddField {
            collection: "tasks".into(),
            name: "amount".into(),
            ty: FieldType::nullable(FieldType::IntNum),
            indexed: true,
            required: true,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"op\":\"add_field\""), "{s}");
        let back: SchemaChange = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn registry_roundtrips_json() {
        let r = tasks_registry();
        let s = serde_json::to_string(&r).unwrap();
        let back: SchemaRegistry = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
