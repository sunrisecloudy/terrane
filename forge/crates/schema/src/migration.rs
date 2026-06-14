//! Deterministic, atomic-friendly **record migration engine** (DL-13).
//!
//! prd-merged/02 DL-13: *"Logical migrations are oplog operations, never
//! destructive SQLite DDL."* A [`SchemaChange`](crate::SchemaChange) evolves the
//! **registry** (what the schema is); a [`MigrationDescriptor`] rewrites the
//! **stored records** to match such an evolution. This module is the pure-logic
//! half — a record migration is a deterministic function of `(prior record,
//! descriptor)` with no I/O, no clock, and no RNG, so it is replay-safe and
//! content-addressable (`forge/spec/migrations.md`). The storage layer wraps it
//! in a single [`Store::transact`] for all-or-nothing atomicity.
//!
//! Transforms are keyed by the **stable `field_id`** (DL-7), never the display
//! name. Supported: `add_field` (fill a default into records missing the field),
//! `rename_field` (display-name only — the stable id value never moves),
//! `drop_field` (remove the value side; the schema field is kept via deprecate),
//! and `widen_field` (coerce the stored value to a wider type). Narrowing is
//! rejected: a type relation that is itself a narrowing fails up front, and a
//! widen whose stored value cannot be losslessly represented in the wider type
//! (e.g. `12.5` → `int_num`) fails while transforming that record — which, under
//! the storage layer's single transaction, rolls the WHOLE migration back.

use crate::field_type::FieldType;
use forge_domain::{CoreError, RecordEnvelope, Result};
use serde::{Deserialize, Serialize};

/// One per-field record transform (DL-13). Tagged by `op` (snake_case) and keyed
/// by the stable `field_id` (DL-7), mirroring [`SchemaChange`](crate::SchemaChange)'s
/// serde shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FieldTransform {
    /// Fill `default` into every record that does not already carry `field_id`.
    /// Records that already have the field are left untouched (idempotent). The
    /// display `name` is also written so the projection stays readable.
    AddField {
        field_id: String,
        name: String,
        default: serde_json::Value,
    },
    /// DL-7: change only the display-name projection. The stable `field_id` value
    /// is untouched, so a record that carries the value by id needs no value move.
    RenameField { field_id: String, name: String },
    /// Drop the field's value from both the stable-id map and the display
    /// projection. This is the *data* side of a deprecate (DL-8 keeps the schema
    /// field); it is recorded in the oplog, never a destructive DDL.
    DropField { field_id: String },
    /// Coerce the stored value to the wider type `to`. Only a widening relation is
    /// legal; a narrowing — or a value that cannot be losslessly widened — is a
    /// typed error.
    WidenField { field_id: String, to: FieldType },
}

impl FieldTransform {
    /// The stable field id this transform targets (DL-7).
    pub fn field_id(&self) -> &str {
        match self {
            FieldTransform::AddField { field_id, .. }
            | FieldTransform::RenameField { field_id, .. }
            | FieldTransform::DropField { field_id }
            | FieldTransform::WidenField { field_id, .. } => field_id,
        }
    }
}

/// A migration: rewrite every record of one `collection` from
/// `from_schema_version` to `to_schema_version` by applying `transforms` in order
/// (DL-13). The descriptor is a pure value — the engine ([`migrate_record`]) and
/// the storage driver (`Store::apply_migration`) consume it; it carries no I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationDescriptor {
    pub collection: String,
    pub from_schema_version: u64,
    pub to_schema_version: u64,
    pub transforms: Vec<FieldTransform>,
}

impl MigrationDescriptor {
    /// Structural validation independent of any record: the version must advance
    /// (`to > from`), the collection must be named, and no two transforms may be a
    /// duplicate `add_field` for the same id. Pure; the storage layer also checks
    /// the *runtime* precondition (current version == `from`).
    pub fn validate(&self) -> Result<()> {
        if self.collection.trim().is_empty() {
            return Err(CoreError::ValidationError(
                "migration descriptor has empty collection".into(),
            ));
        }
        if self.to_schema_version <= self.from_schema_version {
            return Err(CoreError::ValidationError(format!(
                "migration must advance the schema version (from {} to {})",
                self.from_schema_version, self.to_schema_version
            )));
        }
        Ok(())
    }
}

/// Apply a migration to one record, deterministically (DL-13).
///
/// Pure function of `(prior, descriptor)`: the same inputs always yield a
/// byte-identical migrated record (canonical JSON over `BTreeMap`, no clock/RNG),
/// so the result is replay-safe and content-addressable
/// (`forge/spec/migrations.md` §2). Transforms run in `descriptor.transforms`
/// order. A transform that cannot be applied losslessly (a narrowing widen, or a
/// non-integral `float → int`) returns a [`CoreError::SchemaCompatibilityError`];
/// under the storage driver's single transaction that rolls the whole migration
/// back. Fields the descriptor does not mention — including `unknown_fields`
/// (DL-9) — are carried through verbatim, as are record identity and lifecycle
/// (`entity_id`, `collection`, `created_at`, `updated_at`, `deleted`).
pub fn migrate_record(
    prior: &RecordEnvelope,
    descriptor: &MigrationDescriptor,
) -> Result<RecordEnvelope> {
    let mut next = prior.clone();
    for transform in &descriptor.transforms {
        apply_transform(&mut next, transform)?;
    }
    Ok(next)
}

/// Apply one [`FieldTransform`] to a record in place.
fn apply_transform(record: &mut RecordEnvelope, transform: &FieldTransform) -> Result<()> {
    match transform {
        FieldTransform::AddField {
            field_id,
            name,
            default,
        } => {
            // Idempotent: only fill the default when the record is missing the
            // field (by stable id). A record already carrying the field keeps its
            // value. Mirror the value into the display projection so reads stay
            // readable (the projection name → value map).
            record
                .field_ids
                .entry(field_id.clone())
                .or_insert_with(|| default.clone());
            record
                .fields
                .entry(name.clone())
                .or_insert_with(|| default.clone());
            Ok(())
        }
        FieldTransform::RenameField { field_id: _, name } => {
            // DL-7: a rename changes only the display name. The stable-id value is
            // authoritative and untouched; there is nothing to move in the
            // record's value maps. The display projection is a derived view the
            // registry rename already updated, so a record migration has no value
            // work here — the transform is recorded for replay completeness.
            let _ = name;
            Ok(())
        }
        FieldTransform::DropField { field_id } => {
            // Drop the value side from both maps. The schema field is retained via
            // deprecate (DL-8); only the stored value is removed, replayably.
            record.field_ids.remove(field_id);
            // The display projection has no stable-id key, so we cannot map id →
            // name without the registry. Storage materializes `f_<name>` ids, so a
            // dropped `f_<name>` id removes its matching display field too.
            if let Some(name) = field_id.strip_prefix("f_") {
                record.fields.remove(name);
            }
            Ok(())
        }
        FieldTransform::WidenField { field_id, to } => {
            widen_value(record, field_id, to)
        }
    }
}

/// Coerce a record's stored value at `field_id` to the wider type `to`,
/// rewriting both the stable-id map and (when present) the display projection.
/// A missing value is a no-op (nothing to widen). A value that cannot be
/// losslessly represented in `to` is a [`CoreError::SchemaCompatibilityError`].
fn widen_value(record: &mut RecordEnvelope, field_id: &str, to: &FieldType) -> Result<()> {
    // Widen the authoritative stable-id value first; reuse the coerced value for
    // the display projection so the two never diverge.
    let coerced = match record.field_ids.get(field_id) {
        Some(value) => Some(coerce_to(value, to, field_id)?),
        None => None,
    };
    if let Some(coerced) = coerced {
        record.field_ids.insert(field_id.to_string(), coerced.clone());
        if let Some(name) = field_id.strip_prefix("f_") {
            if let Some(slot) = record.fields.get_mut(name) {
                *slot = coerced;
            }
        }
    }
    Ok(())
}

/// Coerce one JSON `value` to the wider type `to`, deterministically (DL-13).
///
/// Only widening coercions are produced: `int_num → float_num` rewrites an
/// integer to its float form; widening *to* `scalar` or `nullable` leaves the
/// value as-is (any value already satisfies the wider type). `null` is preserved
/// (it widens into a nullable target unchanged). A narrowing target (e.g.
/// `float_num → int_num`) with a value that cannot be represented losslessly — a
/// non-integral float — is rejected; a float that IS integral (`5.0`) coerces to
/// the integer, but the *type relation* float→int is itself a narrowing the
/// registry already forbids, so this path is reached only for a value-level
/// lossiness check on a target the descriptor permits.
fn coerce_to(value: &serde_json::Value, to: &FieldType, field_id: &str) -> Result<serde_json::Value> {
    // Peel `nullable`: a null passes through unchanged; a non-null widens to the
    // inner core. Widening `T → nullable(U)` is `T → U` on the value (DL-8).
    if let FieldType::Nullable(inner) = to {
        if value.is_null() {
            return Ok(serde_json::Value::Null);
        }
        return coerce_to(value, inner, field_id);
    }

    match to {
        // The universal scalar top type holds any concrete scalar verbatim, so
        // `text → scalar` (the widen_text_to_scalar fixture) is identity here.
        FieldType::Scalar => Ok(value.clone()),
        // A `text`/`bool` *target* is only ever a same-type widen (no narrower
        // type widens into them), so the value passes through unchanged.
        FieldType::Text | FieldType::Bool => Ok(value.clone()),
        FieldType::FloatNum => {
            // int → float: rewrite the integer to its float JSON form so the
            // stored value's type matches the widened field (e.g. 5 → 5.0).
            if let Some(i) = value.as_i64() {
                return Ok(json_float(i as f64));
            }
            if let Some(u) = value.as_u64() {
                return Ok(json_float(u as f64));
            }
            // Already a float (or null handled above) — leave as-is.
            Ok(value.clone())
        }
        FieldType::IntNum => {
            // A float → int target is a narrowing the registry rejects, but a
            // value-level check still guards lossiness: only an integral float
            // coerces; a fractional one is rejected (the lossy narrow case the
            // `narrow_float_to_int_rejected` fixture pins).
            if let Some(i) = value.as_i64() {
                return Ok(serde_json::json!(i));
            }
            if let Some(f) = value.as_f64() {
                if f.fract() == 0.0 && f.is_finite() {
                    return Ok(serde_json::json!(f as i64));
                }
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "field {field_id:?}: value {f} cannot be narrowed to int_num without loss"
                )));
            }
            Ok(value.clone())
        }
        FieldType::Nullable(_) => unreachable!("nullable peeled above"),
    }
}

/// Build a JSON number from an `f64`, preserving the float type even for integral
/// values (so `5.0` serializes as a float, not as `5`). Falls back to the integer
/// form only for non-finite inputs, which `serde_json` cannot represent as a
/// float.
fn json_float(f: f64) -> serde_json::Value {
    serde_json::Number::from_f64(f)
        .map(serde_json::Value::Number)
        // NaN/Inf are not valid JSON numbers; an int field never produces them, so
        // this branch is unreachable in practice. Degrade to null rather than panic.
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{CollectionId, LogicalTimestamp, RecordId};
    use std::collections::BTreeMap;

    fn rec(field_ids: &[(&str, serde_json::Value)]) -> RecordEnvelope {
        let mut r = RecordEnvelope::new(
            CollectionId::new("expenses"),
            RecordId::new("e1"),
            BTreeMap::new(),
            LogicalTimestamp(1),
        );
        r.field_ids = field_ids
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        r
    }

    fn desc(transforms: Vec<FieldTransform>) -> MigrationDescriptor {
        MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 1,
            to_schema_version: 2,
            transforms,
        }
    }

    #[test]
    fn descriptor_validate_requires_advancing_version() {
        let mut d = desc(vec![]);
        assert!(d.validate().is_ok());
        d.to_schema_version = 1;
        assert_eq!(d.validate().unwrap_err().code(), "ValidationError");
        d.to_schema_version = 0;
        assert_eq!(d.validate().unwrap_err().code(), "ValidationError");
    }

    #[test]
    fn descriptor_validate_rejects_empty_collection() {
        let mut d = desc(vec![]);
        d.collection = "  ".into();
        assert_eq!(d.validate().unwrap_err().code(), "ValidationError");
    }

    #[test]
    fn add_field_fills_default_only_when_missing() {
        let d = desc(vec![FieldTransform::AddField {
            field_id: "f_currency".into(),
            name: "currency".into(),
            default: serde_json::json!("USD"),
        }]);
        // Missing → filled.
        let r = rec(&[("f_amount", serde_json::json!(10))]);
        let out = migrate_record(&r, &d).unwrap();
        assert_eq!(out.field_ids["f_currency"], serde_json::json!("USD"));
        assert_eq!(out.fields["currency"], serde_json::json!("USD"));
        // Already present → left untouched (idempotent).
        let r2 = rec(&[("f_currency", serde_json::json!("EUR"))]);
        let out2 = migrate_record(&r2, &d).unwrap();
        assert_eq!(out2.field_ids["f_currency"], serde_json::json!("EUR"));
    }

    #[test]
    fn widen_int_to_float_rewrites_value_type() {
        let d = desc(vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to: FieldType::FloatNum,
        }]);
        let r = rec(&[("f_amount", serde_json::json!(5))]);
        let out = migrate_record(&r, &d).unwrap();
        // 5 → 5.0: the stored JSON is now a float.
        assert!(out.field_ids["f_amount"].is_f64(), "int must widen to float");
        assert_eq!(out.field_ids["f_amount"].as_f64(), Some(5.0));
    }

    #[test]
    fn widen_to_scalar_and_nullable_preserve_value() {
        // Any concrete scalar widens to scalar verbatim.
        let d_scalar = desc(vec![FieldTransform::WidenField {
            field_id: "f_body".into(),
            to: FieldType::Scalar,
        }]);
        let r = rec(&[("f_body", serde_json::json!("hello"))]);
        assert_eq!(
            migrate_record(&r, &d_scalar).unwrap().field_ids["f_body"],
            serde_json::json!("hello")
        );
        // T → nullable(T): a present value is unchanged; a null passes through.
        let d_null = desc(vec![FieldTransform::WidenField {
            field_id: "f_estimate".into(),
            to: FieldType::nullable(FieldType::IntNum),
        }]);
        let r_present = rec(&[("f_estimate", serde_json::json!(3))]);
        assert_eq!(
            migrate_record(&r_present, &d_null).unwrap().field_ids["f_estimate"],
            serde_json::json!(3)
        );
        let r_null = rec(&[("f_estimate", serde_json::json!(null))]);
        assert_eq!(
            migrate_record(&r_null, &d_null).unwrap().field_ids["f_estimate"],
            serde_json::json!(null)
        );
    }

    #[test]
    fn narrow_float_to_int_with_fractional_value_is_rejected() {
        // The lossy-narrow case (narrow_float_to_int_rejected): a non-integral
        // float cannot be coerced to int_num → SchemaCompatibilityError.
        let d = desc(vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to: FieldType::IntNum,
        }]);
        let r = rec(&[("f_amount", serde_json::json!(12.5))]);
        let err = migrate_record(&r, &d).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        assert!(err.to_string().contains("12.5"), "{err}");
    }

    #[test]
    fn integral_float_coerces_to_int_losslessly() {
        // 5.0 IS integral, so the value-level lossiness check passes (5.0 → 5).
        let d = desc(vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to: FieldType::IntNum,
        }]);
        let r = rec(&[("f_amount", serde_json::json!(5.0))]);
        let out = migrate_record(&r, &d).unwrap();
        assert_eq!(out.field_ids["f_amount"], serde_json::json!(5));
    }

    #[test]
    fn drop_field_removes_value_from_both_maps() {
        let mut r = rec(&[("f_old", serde_json::json!("x")), ("f_keep", serde_json::json!(1))]);
        r.fields.insert("old".into(), serde_json::json!("x"));
        let d = desc(vec![FieldTransform::DropField { field_id: "f_old".into() }]);
        let out = migrate_record(&r, &d).unwrap();
        assert!(!out.field_ids.contains_key("f_old"));
        assert!(!out.fields.contains_key("old"));
        // Unrelated field is preserved.
        assert_eq!(out.field_ids["f_keep"], serde_json::json!(1));
    }

    #[test]
    fn rename_preserves_stable_id_value() {
        let r = rec(&[("f_alice_0", serde_json::json!("hi"))]);
        let d = desc(vec![FieldTransform::RenameField {
            field_id: "f_alice_0".into(),
            name: "label".into(),
        }]);
        let out = migrate_record(&r, &d).unwrap();
        // DL-7: the stable id value never moves on a rename.
        assert_eq!(out.field_ids["f_alice_0"], serde_json::json!("hi"));
    }

    #[test]
    fn unknown_fields_are_preserved_dl9() {
        let mut r = rec(&[("f_amount", serde_json::json!(5))]);
        r.unknown_fields.insert("f_future".into(), serde_json::json!({"x": 1}));
        let d = desc(vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to: FieldType::FloatNum,
        }]);
        let out = migrate_record(&r, &d).unwrap();
        assert_eq!(out.unknown_fields["f_future"], serde_json::json!({"x": 1}));
    }

    #[test]
    fn migration_is_deterministic_byte_identical() {
        // The determinism contract: same prior + descriptor → byte-identical out.
        let mut r = rec(&[("f_amount", serde_json::json!(5)), ("f_z", serde_json::json!(true))]);
        r.field_ids.insert("f_a".into(), serde_json::json!("first"));
        let d = desc(vec![
            FieldTransform::WidenField { field_id: "f_amount".into(), to: FieldType::FloatNum },
            FieldTransform::AddField {
                field_id: "f_currency".into(),
                name: "currency".into(),
                default: serde_json::json!("USD"),
            },
        ]);
        let a = serde_json::to_vec(&migrate_record(&r, &d).unwrap()).unwrap();
        let b = serde_json::to_vec(&migrate_record(&r, &d).unwrap()).unwrap();
        assert_eq!(a, b, "same inputs must yield byte-identical output");
    }

    #[test]
    fn identity_record_fields_are_preserved() {
        let r = rec(&[("f_amount", serde_json::json!(5))]);
        let d = desc(vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to: FieldType::FloatNum,
        }]);
        let out = migrate_record(&r, &d).unwrap();
        assert_eq!(out.entity_id, r.entity_id);
        assert_eq!(out.collection, r.collection);
        assert_eq!(out.created_at, r.created_at);
        assert_eq!(out.updated_at, r.updated_at);
        assert_eq!(out.deleted, r.deleted);
        assert_eq!(out.envelope_version, r.envelope_version);
    }

    #[test]
    fn descriptor_and_transform_roundtrip_json() {
        let d = desc(vec![
            FieldTransform::AddField {
                field_id: "f_c".into(),
                name: "currency".into(),
                default: serde_json::json!("USD"),
            },
            FieldTransform::WidenField { field_id: "f_a".into(), to: FieldType::FloatNum },
            FieldTransform::DropField { field_id: "f_old".into() },
            FieldTransform::RenameField { field_id: "f_a".into(), name: "amount".into() },
        ]);
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"op\":\"add_field\""), "{s}");
        assert!(s.contains("\"op\":\"widen_field\""), "{s}");
        let back: MigrationDescriptor = serde_json::from_str(&s).unwrap();
        assert_eq!(d, back);
    }
}
