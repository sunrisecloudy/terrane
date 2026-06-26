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

    /// The **least upper bound** (join) of two field types over the widen
    /// lattice ([`Self::can_widen_to`]): the *narrowest* type both `self` and
    /// `other` widen to, or `None` when no additive common supertype exists.
    ///
    /// This is the type-merge primitive the CRDT registry union relies on
    /// (DL-11 review 160). When two divergent branches widened the SAME stable
    /// field id in different directions — e.g. an `IntNum` base that peer A
    /// widened to `FloatNum` and peer B widened to `Nullable(IntNum)` — both
    /// directions are legal individually, but neither directly widens to the
    /// other, so a `order_key` "larger wins" tie-break could pick a type that is
    /// NOT a supertype of the loser and thereby silently *narrow* the loser's
    /// branch. The join instead returns the genuine common supertype
    /// (`Nullable(FloatNum)` for that example), so the merged schema can still
    /// describe every value either branch admitted.
    ///
    /// The join is a true semilattice meet-from-below over the widen partial
    /// order — **commutative**, **associative**, and **idempotent** — so the
    /// registry merge converges to the same type regardless of delivery order.
    /// For every `Some(j)` it returns, both `self.can_widen_to(&j)` and
    /// `other.can_widen_to(&j)` hold (it is an upper bound of both inputs);
    /// `None` means there is genuinely no additive common supertype (e.g.
    /// `Bool` vs `Text`), and the caller must fail closed (reject / keep local)
    /// rather than narrow.
    ///
    /// Rules (all derived from [`Self::can_widen_to`]):
    /// - `T` join `T` = `T` (idempotent);
    /// - `IntNum` join `FloatNum` = `FloatNum` (numeric widening);
    /// - any two *distinct* concrete scalars whose only common supertype is the
    ///   universal top join to `Scalar` (`Bool` join `Text` = `Scalar`, etc.);
    /// - nullable absorbs: `Nullable(T)` join `U` = `Nullable(T join U)` on
    ///   either side, so adding a null layer on one branch keeps the result
    ///   nullable (e.g. `FloatNum` join `Nullable(IntNum)` = `Nullable(FloatNum)`,
    ///   `Scalar` join `Nullable(T)` = `Nullable(Scalar)`).
    pub fn join(&self, other: &FieldType) -> Option<FieldType> {
        field_type_join(self, other)
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

/// Least upper bound (join) of two field types over the widen lattice — the
/// narrowest type both inputs widen to, or `None` when there is no additive
/// common supertype. See [`FieldType::join`] for the contract; this is the
/// commutative/associative/idempotent semilattice operation the CRDT registry
/// union merge uses to resolve a divergently-widened shared field (DL-11 review
/// 160) without ever narrowing either branch.
pub(crate) fn field_type_join(a: &FieldType, b: &FieldType) -> Option<FieldType> {
    // Split each side into (nullable?, non-null core). `inner()` peels EVERY
    // nullable layer, so the cores are always non-nullable and the result is
    // re-wrapped at most ONCE — keeping the join canonical (a single null layer)
    // so it stays associative regardless of grouping (`Nullable(Nullable(T))`
    // can never leak in).
    let nullable = a.is_nullable() || b.is_nullable();
    let core = scalar_join(a.inner(), b.inner())?;
    Some(if nullable { FieldType::nullable(core) } else { core })
}

/// Join of two NON-nullable cores over the scalar widen chain. Returns `None`
/// only when there is genuinely no additive common supertype (not reachable for
/// the current scalar-only type set, where `Scalar` is the universal top, but
/// kept as the fail-closed seam for future non-scalar types).
fn scalar_join(a: &FieldType, b: &FieldType) -> Option<FieldType> {
    use FieldType::*;
    debug_assert!(!a.is_nullable() && !b.is_nullable(), "scalar_join expects non-null cores");
    match (a, b) {
        // Identity / one directly widens to the other: the wider one is the lub
        // (covers T join T and IntNum→FloatNum / scalar→Scalar in either order).
        _ if a == b => Some(a.clone()),
        _ if a.can_widen_to(b) => Some(b.clone()),
        _ if b.can_widen_to(a) => Some(a.clone()),
        // Two distinct numeric cores share `FloatNum` (the narrower widening).
        (IntNum | FloatNum, IntNum | FloatNum) => Some(FloatNum),
        // Any other distinct scalar pair tops out at the universal `Scalar`.
        (
            IntNum | FloatNum | Bool | Text | Scalar,
            IntNum | FloatNum | Bool | Text | Scalar,
        ) => Some(Scalar),
        // No additive common supertype (future non-scalar types fall here).
        _ => None,
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

    // A small, representative type universe for the join algebra properties.
    fn join_universe() -> Vec<FieldType> {
        vec![
            FieldType::IntNum,
            FieldType::FloatNum,
            FieldType::Bool,
            FieldType::Text,
            FieldType::Scalar,
            FieldType::nullable(FieldType::IntNum),
            FieldType::nullable(FieldType::FloatNum),
            FieldType::nullable(FieldType::Bool),
            FieldType::nullable(FieldType::Scalar),
        ]
    }

    #[test]
    fn join_idempotent_on_identity() {
        // T join T = T.
        for t in join_universe() {
            assert_eq!(t.join(&t), Some(t.clone()), "{t:?} join itself must be itself");
        }
    }

    #[test]
    fn join_numeric_widening() {
        // IntNum join FloatNum = FloatNum (the numeric widening direction).
        assert_eq!(FieldType::IntNum.join(&FieldType::FloatNum), Some(FieldType::FloatNum));
        assert_eq!(FieldType::FloatNum.join(&FieldType::IntNum), Some(FieldType::FloatNum));
    }

    #[test]
    fn join_distinct_scalars_top_out_at_scalar() {
        // Two unrelated concrete scalars (no direct widening) join to the
        // universal `Scalar` top — never None, since Scalar admits any value.
        assert_eq!(FieldType::Bool.join(&FieldType::Text), Some(FieldType::Scalar));
        assert_eq!(FieldType::IntNum.join(&FieldType::Bool), Some(FieldType::Scalar));
        // Any concrete scalar joined with Scalar is Scalar (Scalar is the top).
        for t in [FieldType::IntNum, FieldType::FloatNum, FieldType::Bool, FieldType::Text] {
            assert_eq!(t.join(&FieldType::Scalar), Some(FieldType::Scalar), "{t:?} join Scalar");
        }
    }

    #[test]
    fn join_adds_nullable_when_either_side_is_nullable() {
        // T join Nullable(T) = Nullable(T) (adding null tolerance is additive).
        assert_eq!(
            FieldType::IntNum.join(&FieldType::nullable(FieldType::IntNum)),
            Some(FieldType::nullable(FieldType::IntNum))
        );
        // Scalar join Nullable(T) = Nullable(Scalar) — null layer absorbs, core
        // tops out at Scalar.
        assert_eq!(
            FieldType::Scalar.join(&FieldType::nullable(FieldType::Bool)),
            Some(FieldType::nullable(FieldType::Scalar))
        );
    }

    #[test]
    fn join_divergent_widening_keeps_a_supertype_of_both() {
        // THE review-160 case: an IntNum base that peer A widened to FloatNum and
        // peer B widened to Nullable(IntNum). Neither directly widens to the other,
        // but the join is Nullable(FloatNum) — a genuine supertype of BOTH (not the
        // order_key-larger Nullable(IntNum), which would narrow the float branch).
        let a = FieldType::FloatNum;
        let b = FieldType::nullable(FieldType::IntNum);
        let j = a.join(&b).expect("a common supertype exists");
        assert_eq!(j, FieldType::nullable(FieldType::FloatNum));
        assert!(a.can_widen_to(&j), "join must be an upper bound of FloatNum");
        assert!(b.can_widen_to(&j), "join must be an upper bound of Nullable(IntNum)");
    }

    #[test]
    fn join_is_commutative_idempotent_and_an_upper_bound() {
        // Property-ish over the small universe: for every pair, the join (when it
        // exists) is commutative AND an upper bound of both inputs.
        let types = join_universe();
        for a in &types {
            for b in &types {
                let ab = a.join(b);
                let ba = b.join(a);
                assert_eq!(ab, ba, "join must be commutative: {a:?} vs {b:?}");
                if let Some(j) = ab {
                    assert!(
                        a.can_widen_to(&j) && b.can_widen_to(&j),
                        "join {j:?} must be an upper bound of {a:?} and {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn join_is_associative() {
        // (a join b) join c == a join (b join c) wherever both sides exist — the
        // semilattice law that makes the registry merge order-independent.
        let types = join_universe();
        for a in &types {
            for b in &types {
                for c in &types {
                    let left = a.join(b).and_then(|ab| ab.join(c));
                    let right = b.join(c).and_then(|bc| a.join(&bc));
                    assert_eq!(left, right, "join must be associative: {a:?}, {b:?}, {c:?}");
                }
            }
        }
    }

    #[test]
    fn field_type_roundtrips_json() {
        let t = FieldType::nullable(FieldType::IntNum);
        let s = serde_json::to_string(&t).unwrap();
        let back: FieldType = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }
}
