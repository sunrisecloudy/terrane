//! Field types + the additive-only widening relation (prd-merged/02 DL-8).
//!
//! A field's `FieldType` selects (eventually) its merge semantics (DL-3) and,
//! more importantly for this crate, governs which schema evolutions are
//! *forward-compatible*: a type may only ever be **widened** to a type that can
//! represent every prior value (DL-8 "widen type"). Narrowing is destructive
//! and is rejected by [`FieldType::can_widen_to`].

use serde::{Deserialize, Serialize};

/// The set of field value types the M0a spine understands.
///
/// `nullable` is modelled as a wrapper variant so the canonical "scalar →
/// nullable" widening (DL-8) is expressible without a separate `nullable: bool`
/// flag that would have to be threaded through every comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// Free-form scalar JSON value (the most permissive non-null type). Any
    /// concrete scalar (`IntNum`, `FloatNum`, `Bool`, `Text`) can widen *into*
    /// this, since it can hold any of their values.
    Scalar,
    /// UTF-8 text. In the full data layer this maps to collaborative text
    /// (DL-3); for schema purposes it is just a distinct scalar type.
    Text,
    /// 64-bit signed integer.
    IntNum,
    /// 64-bit float. `IntNum` widens to this (every i64 the spine stores is
    /// representable; DL-8 "Int → Float").
    FloatNum,
    /// Boolean.
    Bool,
    /// A value that may also be `null`. Widening `T` → `Nullable(T)` is the
    /// canonical "scalar → nullable" additive change (DL-8): existing non-null
    /// data stays valid, the column simply also admits absence.
    Nullable(Box<FieldType>),
}

impl FieldType {
    /// Convenience constructor for the nullable wrapper.
    pub fn nullable(inner: FieldType) -> FieldType {
        FieldType::Nullable(Box::new(inner))
    }

    /// True if this type is (possibly nested) nullable.
    pub fn is_nullable(&self) -> bool {
        matches!(self, FieldType::Nullable(_))
    }

    /// The non-nullable core of this type (peels one or more `Nullable` layers).
    pub fn inner(&self) -> &FieldType {
        match self {
            FieldType::Nullable(t) => t.inner(),
            other => other,
        }
    }

    /// Whether a field of `self` may be **widened** to `target` without
    /// invalidating any value already stored as `self` (prd-merged/02 DL-8).
    ///
    /// The relation is reflexive (no-op widen is allowed) and forms the only
    /// permitted type evolution. It is intentionally *not* symmetric: the
    /// reverse direction is a narrowing and therefore destructive.
    ///
    /// Rules:
    /// - any type widens to itself;
    /// - `IntNum` → `FloatNum` (DL-8 numeric widening);
    /// - any concrete scalar widens to `Scalar` (the universal scalar top);
    /// - `T` → `Nullable(U)` iff `T` widens to `U` (adding null tolerance is
    ///   additive — existing non-null values still satisfy `Nullable(U)`);
    /// - `Nullable(T)` → `Nullable(U)` iff `T` → `U`;
    /// - `Nullable(T)` → `U` (non-nullable) is a **narrowing** (drops the null
    ///   case) and is rejected.
    pub fn can_widen_to(&self, target: &FieldType) -> bool {
        // Identity widen.
        if self == target {
            return true;
        }
        match (self, target) {
            // Nullable → Nullable: widen iff the inner cores widen. (Checked
            // before the general "add null tolerance" arm so a nullable source
            // is not misread as narrowing when peeling the target's layer.)
            (FieldType::Nullable(src_inner), FieldType::Nullable(tgt_inner)) => {
                src_inner.can_widen_to(tgt_inner)
            }
            // Adding null tolerance to a non-nullable source: the source must
            // widen to the target's core.
            (src, FieldType::Nullable(inner)) => src.can_widen_to(inner),
            // Removing null tolerance is narrowing — not allowed.
            (FieldType::Nullable(_), _) => false,
            // Numeric widening.
            (FieldType::IntNum, FieldType::FloatNum) => true,
            // Any concrete scalar widens to the universal `Scalar` top type.
            (
                FieldType::IntNum | FieldType::FloatNum | FieldType::Bool | FieldType::Text,
                FieldType::Scalar,
            ) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widen_is_reflexive() {
        for t in [
            FieldType::Scalar,
            FieldType::Text,
            FieldType::IntNum,
            FieldType::FloatNum,
            FieldType::Bool,
            FieldType::nullable(FieldType::IntNum),
        ] {
            assert!(t.can_widen_to(&t), "{t:?} should widen to itself");
        }
    }

    #[test]
    fn int_widens_to_float_but_not_back() {
        assert!(FieldType::IntNum.can_widen_to(&FieldType::FloatNum));
        assert!(!FieldType::FloatNum.can_widen_to(&FieldType::IntNum));
    }

    #[test]
    fn concrete_scalars_widen_to_universal_scalar() {
        for t in [
            FieldType::IntNum,
            FieldType::FloatNum,
            FieldType::Bool,
            FieldType::Text,
        ] {
            assert!(t.can_widen_to(&FieldType::Scalar), "{t:?} -> Scalar");
        }
        // Scalar does not narrow back to a concrete type.
        assert!(!FieldType::Scalar.can_widen_to(&FieldType::IntNum));
    }

    #[test]
    fn adding_nullable_is_widening() {
        assert!(FieldType::IntNum.can_widen_to(&FieldType::nullable(FieldType::IntNum)));
        // And Int -> Float -> Nullable composes.
        assert!(FieldType::IntNum.can_widen_to(&FieldType::nullable(FieldType::FloatNum)));
    }

    #[test]
    fn removing_nullable_is_narrowing() {
        assert!(!FieldType::nullable(FieldType::IntNum).can_widen_to(&FieldType::IntNum));
    }

    #[test]
    fn nullable_widens_when_inner_widens() {
        assert!(FieldType::nullable(FieldType::IntNum)
            .can_widen_to(&FieldType::nullable(FieldType::FloatNum)));
        assert!(!FieldType::nullable(FieldType::FloatNum)
            .can_widen_to(&FieldType::nullable(FieldType::IntNum)));
    }

    #[test]
    fn inner_peels_all_nullable_layers() {
        let t = FieldType::nullable(FieldType::nullable(FieldType::Bool));
        assert_eq!(t.inner(), &FieldType::Bool);
        assert!(t.is_nullable());
        assert!(!FieldType::Bool.is_nullable());
    }

    #[test]
    fn unrelated_types_do_not_widen() {
        assert!(!FieldType::Bool.can_widen_to(&FieldType::Text));
        assert!(!FieldType::Text.can_widen_to(&FieldType::IntNum));
    }

    #[test]
    fn field_type_roundtrips_json() {
        let t = FieldType::nullable(FieldType::IntNum);
        let s = serde_json::to_string(&t).unwrap();
        let back: FieldType = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }
}
