//! Field and collection definitions.
//!
//! prd-merged/02 DL-7 (stable `field_id`, never reused; renames touch only the
//! display name; **per-actor id ranges: actor-id ⊕ counter**), DL-8 (additive
//! evolution), DL-11 (registry versions are CRDT vectors — two offline actors
//! adding fields merge to the union *by construction*, which requires their
//! freshly minted ids to be distinct), DL-12 (constraints warn before enforce).

use crate::field_type::FieldType;
use forge_domain::ActorId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single field in a collection's schema.
///
/// The `field_id` is **stable and never reused** (DL-7): a rename changes only
/// `name`. `deprecated` hides a field from new writes/UI without removing it
/// (DL-8 "deprecate = hide", "delete = deprecate + retain"), so old data keeps
/// round-tripping.
///
/// Internals are private (review 005 P2): external crates can only *read* a
/// field and must go through [`crate::SchemaChange`] to mutate one, so they
/// cannot bypass the additive-only / id-stability invariants. Construction
/// outside this crate goes through the serde `Deserialize` path (the registry
/// re-validates on deserialize via [`crate::SchemaRegistry::validated`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    /// Stable identifier, e.g. `f_<actor>_<seq>`. Allocated once, never reused
    /// (DL-7); the actor prefix makes ids minted offline by distinct actors
    /// collision-free (DL-11).
    field_id: String,
    /// Display name. The only thing a rename touches (DL-7).
    name: String,
    /// Value type; may only ever widen (DL-8, see [`FieldType::can_widen_to`]).
    ty: FieldType,
    /// Whether the projection should build an expression index (DL-5).
    indexed: bool,
    /// Hidden from new writes/UI but retained for read (DL-8).
    deprecated: bool,
    /// Whether a value is required on write (DL-12). Only *enforced* once
    /// `enforced` flips true; until then a missing value is a warning.
    required: bool,
    /// Whether the `required` constraint is in enforcement mode. New
    /// constraints start in warning mode (`false`) per DL-12 ("new constraints
    /// default to warning mode before enforcement mode").
    enforced: bool,
}

impl FieldDef {
    // ------------------------------------------------------------- accessors

    /// Stable, never-reused field id (DL-7).
    pub fn field_id(&self) -> &str {
        &self.field_id
    }
    /// Current display name (mutable only via a rename `SchemaChange`).
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Declared value type.
    pub fn ty(&self) -> &FieldType {
        &self.ty
    }
    /// Whether the projection should build an expression index (DL-5).
    pub fn indexed(&self) -> bool {
        self.indexed
    }
    /// Whether the field is hidden-but-retained (DL-8).
    pub fn deprecated(&self) -> bool {
        self.deprecated
    }
    /// Whether a value is declared required (DL-12).
    pub fn required(&self) -> bool {
        self.required
    }
    /// Whether the `required` constraint is in enforcement mode (DL-12).
    pub fn enforced(&self) -> bool {
        self.enforced
    }

    /// True if this field's `required` constraint should *error* (rather than
    /// merely warn) on a missing value: it must be both required and enforced,
    /// and not deprecated (a deprecated field is never required-for-write).
    pub fn requires_value(&self) -> bool {
        self.required && self.enforced && !self.deprecated
    }

    // ------------------------------------------- crate-internal mutation hooks

    pub(crate) fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub(crate) fn set_type(&mut self, ty: FieldType) {
        self.ty = ty;
    }
    pub(crate) fn set_deprecated(&mut self, deprecated: bool) {
        self.deprecated = deprecated;
    }
    pub(crate) fn set_enforced(&mut self, enforced: bool) {
        self.enforced = enforced;
    }

    /// Deterministically merge two definitions of the SAME stable `field_id` from
    /// two divergent registry branches into their least-upper-bound (DL-11 review
    /// 159/160). Both inputs must share `field_id` (the caller keys the union by
    /// it); the result is **commutative** (`merge(a, b) == merge(b, a)`) and
    /// **idempotent** (`merge(a, a) == a`), so two peers converge regardless of
    /// delivery order. Each component is resolved monotonically:
    ///
    /// - **type**: the field-type **join** (least upper bound) of the two under
    ///   the widen lattice ([`FieldType::join`]). The join is the *narrowest*
    ///   common supertype, so it never rolls a wider local field back (review
    ///   147) AND never silently narrows a divergent widening (review 160): when
    ///   peer A widened an `IntNum` to `FloatNum` while peer B widened the same
    ///   field to `Nullable(IntNum)`, the join is `Nullable(FloatNum)` — a real
    ///   supertype of BOTH — not the `order_key`-larger `Nullable(IntNum)`, which
    ///   would describe the float branch with a narrower type. When the two types
    ///   are genuinely incompatible (no additive common supertype, e.g. `Bool` vs
    ///   `Text`) the join is `None`; this merge then **fails closed** (`Err`) so
    ///   the caller rejects the whole collection merge / keeps local rather than
    ///   narrowing either branch (commutative: both delivery orders reject);
    /// - **name**: a rename only touches the display name (DL-7), so two branches
    ///   may carry different names; pick the lexicographically larger one
    ///   deterministically (a later duplicate-name collision is caught by
    ///   [`CollectionDef::validate_invariants`] on re-validation);
    /// - **indexed / deprecated / required / enforced**: each is a one-way
    ///   additive flag (turning it on is the only legal transition — DL-8/DL-12),
    ///   so the merge ORs them: once either branch set it, the union keeps it set.
    fn merge_with(&self, other: &FieldDef) -> Result<FieldDef, String> {
        debug_assert_eq!(self.field_id, other.field_id, "merge_with requires the same field_id");
        // JOIN (least-upper-bound) type resolution: commutative, associative, and
        // idempotent over the widen lattice, so it converges regardless of order
        // and is always an upper bound of BOTH inputs (no silent narrowing).
        let ty = self.ty.join(&other.ty).ok_or_else(|| {
            format!(
                "field {:?}: incompatible types {:?} and {:?} have no common supertype \
                 (no additive widening join)",
                self.field_id, self.ty, other.ty
            )
        })?;
        // The join is an upper bound of both inputs: every value either branch
        // admitted is still describable by the merged type (no branch narrowed).
        debug_assert!(
            self.ty.can_widen_to(&ty) && other.ty.can_widen_to(&ty),
            "field-type join {ty:?} must be an upper bound of {:?} and {:?}",
            self.ty,
            other.ty
        );
        // Deterministic display-name resolution: larger name wins (rename only
        // changes the display name; the stable id is shared).
        let name = if other.name > self.name { other.name.clone() } else { self.name.clone() };
        Ok(FieldDef {
            field_id: self.field_id.clone(),
            name,
            ty,
            // One-way additive flags: OR so the union is the least-upper-bound.
            indexed: self.indexed || other.indexed,
            deprecated: self.deprecated || other.deprecated,
            required: self.required || other.required,
            enforced: self.enforced || other.enforced,
        })
    }
}

/// A logical collection (≈ table): an ordered set of [`FieldDef`]s plus the
/// per-actor monotone counters used to mint never-reused field ids (DL-7/DL-11).
///
/// Internals are private (review 005 P2). Mutation only happens through
/// [`crate::SchemaChange`] (applied by [`crate::SchemaRegistry`]); reads go
/// through the accessors below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionDef {
    name: String,
    fields: Vec<FieldDef>,
    /// Per-actor next-sequence map (DL-7 "per-actor id ranges: actor-id ⊕
    /// counter"). Each actor mints ids `f_<actor>_<seq>` from its own counter,
    /// so two actors adding the first field offline get **distinct** ids and
    /// merge to the union by construction (DL-11). Each counter only ever
    /// increases, so an id is never reused.
    #[serde(default)]
    next_field_seq: BTreeMap<String, u64>,
}

impl CollectionDef {
    /// An empty collection named `name`.
    pub(crate) fn new(name: impl Into<String>) -> Self {
        CollectionDef { name: name.into(), fields: Vec::new(), next_field_seq: BTreeMap::new() }
    }

    // ------------------------------------------------------------- accessors

    /// Collection display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Read-only view of the fields in declaration order.
    pub fn fields(&self) -> &[FieldDef] {
        &self.fields
    }

    /// The next sequence number `actor` would consume, without consuming it.
    pub fn next_seq_for(&self, actor: &ActorId) -> u64 {
        self.next_field_seq.get(actor.as_str()).copied().unwrap_or(0)
    }

    /// Snapshot of every actor's next sequence number (used by compatibility
    /// checking to prove no actor's counter went backwards — DL-7).
    pub fn actor_seqs(&self) -> &BTreeMap<String, u64> {
        &self.next_field_seq
    }

    /// The id the next field minted by `actor` will receive, without consuming
    /// the sequence. Pure query — used by callers/tests.
    pub fn peek_next_field_id(&self, actor: &ActorId) -> String {
        format!("f_{}_{}", actor.as_str(), self.next_seq_for(actor))
    }

    /// Look up a field by its stable id.
    pub fn field(&self, field_id: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.field_id == field_id)
    }

    /// Mutable lookup by stable id.
    pub(crate) fn field_mut(&mut self, field_id: &str) -> Option<&mut FieldDef> {
        self.fields.iter_mut().find(|f| f.field_id == field_id)
    }

    /// True if a (non-deprecated or deprecated) field with this display name
    /// already exists — used to reject duplicate names within a collection.
    pub fn has_field_name(&self, name: &str) -> bool {
        self.fields.iter().any(|f| f.name == name)
    }

    /// Allocate a new field for `actor`, minting a fresh actor-scoped stable id
    /// (`f_<actor>_<seq>`) from that actor's counter (DL-7/DL-11).
    ///
    /// `required` constraints always start in **warning mode** (`enforced =
    /// false`) per DL-12; the caller must explicitly enforce later.
    pub(crate) fn add_field(
        &mut self,
        actor: &ActorId,
        name: impl Into<String>,
        ty: FieldType,
        indexed: bool,
        required: bool,
    ) -> &FieldDef {
        let field_id = self.peek_next_field_id(actor);
        let counter = self.next_field_seq.entry(actor.as_str().to_string()).or_insert(0);
        *counter += 1;
        self.fields.push(FieldDef {
            field_id,
            name: name.into(),
            ty,
            indexed,
            deprecated: false,
            required,
            // DL-12: a brand-new required constraint warns before it enforces.
            enforced: false,
        });
        self.fields.last().expect("just pushed")
    }

    /// Deterministically merge this collection with a `carried` definition of the
    /// same logical collection from a divergent registry branch into their
    /// least-upper-bound (the DL-11 CRDT registry merge, review 159). This is the
    /// COMMUTATIVE, IDEMPOTENT field-id union that lets two offline peers — who
    /// each added a different field, or widened the same field independently —
    /// converge to the union regardless of delivery order, instead of one branch's
    /// concurrent-additive work being skipped (review 159) or rolled back (review
    /// 147):
    ///
    /// - **fields**: union keyed by stable `field_id`. A field present in only one
    ///   side is kept as-is; a field present in BOTH is resolved per
    ///   [`FieldDef::merge_with`] (field-type **join** / least-upper-bound, never
    ///   narrows; one-way flags OR; deterministic name). The result is emitted in
    ///   a canonical `field_id`-sorted order so the merge is independent of either
    ///   side's declaration order. If a shared field's two types have no additive
    ///   common supertype (e.g. `Bool` vs `Text`), the merge **fails closed**
    ///   (`Err`) so the caller rejects the import / keeps local rather than
    ///   silently narrowing either branch — commutative, since both delivery
    ///   orders hit the same incompatible pair;
    /// - **actor counters**: the per-actor MAX of the two `next_field_seq` maps, so
    ///   no actor's counter ever regresses (DL-7 — ids are never reused) and the
    ///   merged counters dominate every field id present on either side.
    ///
    /// The collection `name` is taken from `self` (the caller merges same-named
    /// collections; `carried.name` is expected to match). A successful merge still
    /// passes [`Self::validate_invariants`] *unless* the two branches genuinely
    /// conflict on structure (e.g. two live fields renamed onto the same display
    /// name) — the caller re-validates so such a conflict also fails closed rather
    /// than corrupting the schema.
    ///
    /// Pure and deterministic: `merge(a, b) == merge(b, a)` for convergent content
    /// and `merge(a, a) == a`, so a re-merge of the same carried entry is a no-op.
    pub(crate) fn merge_with(&self, carried: &CollectionDef) -> Result<CollectionDef, String> {
        // Union the fields keyed by stable field_id. Start from the local fields,
        // then fold each carried field in: present-in-both resolves via
        // FieldDef::merge_with (which fails closed on an incompatible type pair),
        // carried-only is added.
        let mut by_id: BTreeMap<&str, FieldDef> =
            self.fields.iter().map(|f| (f.field_id(), f.clone())).collect();
        for cf in &carried.fields {
            match by_id.get(cf.field_id()) {
                Some(local) => {
                    let merged = local.merge_with(cf)?;
                    by_id.insert(cf.field_id(), merged);
                }
                None => {
                    by_id.insert(cf.field_id(), cf.clone());
                }
            }
        }
        // Canonical, order-independent field order: by stable field_id (BTreeMap
        // already iterates in sorted key order).
        let fields: Vec<FieldDef> = by_id.into_values().collect();

        // Per-actor MAX of the two counters (least-upper-bound; never regresses).
        let mut next_field_seq = self.next_field_seq.clone();
        for (actor, seq) in &carried.next_field_seq {
            let entry = next_field_seq.entry(actor.clone()).or_insert(0);
            *entry = (*entry).max(*seq);
        }

        Ok(CollectionDef { name: self.name.clone(), fields, next_field_seq })
    }

    /// Re-validate structural invariants after deserialization (review 005 P2):
    /// field ids must be unique within the collection, every field's id must be
    /// reachable by *some* actor counter (no id minted past its counter — an id
    /// `f_<actor>_<n>` requires `next_field_seq[actor] > n`), and display names
    /// must be unique within the collection — so a hand-built/deserialized
    /// registry can't smuggle in a future-colliding id *or* duplicate the
    /// display names that [`add_field`](Self::add_field)/rename reject on the
    /// additive path (review 022 P2).
    pub(crate) fn validate_invariants(&self) -> Result<(), String> {
        let mut seen_ids = std::collections::BTreeSet::new();
        let mut seen_names = std::collections::BTreeSet::new();
        for f in &self.fields {
            if !seen_ids.insert(f.field_id.as_str()) {
                return Err(format!("duplicate field id {:?} in collection {:?}", f.field_id, self.name));
            }
            // Display names are unique within a collection on the additive path
            // (add/rename reject collisions); re-enforce it here so a tampered
            // or deserialized registry can't bypass that invariant (review 022).
            if !seen_names.insert(f.name.as_str()) {
                return Err(format!(
                    "duplicate field name {:?} in collection {:?}",
                    f.name, self.name
                ));
            }
            if let Some((actor, seq)) = parse_field_id(&f.field_id) {
                let next = self.next_field_seq.get(actor).copied().unwrap_or(0);
                if seq >= next {
                    return Err(format!(
                        "field id {:?} in collection {:?} is at/ahead of actor {:?}'s counter \
                         ({} >= {}); id could be reused (DL-7)",
                        f.field_id, self.name, actor, seq, next
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Parse an actor-scoped id `f_<actor>_<seq>` into `(actor, seq)`.
///
/// The actor segment may itself contain underscores, so we split on the *last*
/// underscore for the numeric sequence and strip the leading `f_` prefix for
/// the actor. Ids that don't match the shape return `None` (tolerated — e.g.
/// legacy `f0` ids from an older registry are simply not range-checked).
pub(crate) fn parse_field_id(field_id: &str) -> Option<(&str, u64)> {
    let body = field_id.strip_prefix("f_")?;
    let (actor, seq) = body.rsplit_once('_')?;
    if actor.is_empty() {
        return None;
    }
    let seq: u64 = seq.parse().ok()?;
    Some((actor, seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(s: &str) -> ActorId {
        ActorId::new(s)
    }

    #[test]
    fn add_field_allocates_actor_scoped_stable_ids() {
        let a = actor("alice");
        let mut c = CollectionDef::new("tasks");
        assert_eq!(c.peek_next_field_id(&a), "f_alice_0");
        let id0 = c.add_field(&a, "title", FieldType::Text, false, false).field_id().to_string();
        let id1 = c.add_field(&a, "done", FieldType::Bool, false, false).field_id().to_string();
        assert_eq!(id0, "f_alice_0");
        assert_eq!(id1, "f_alice_1");
        assert_eq!(c.next_seq_for(&a), 2);
        assert_eq!(c.peek_next_field_id(&a), "f_alice_2");
    }

    #[test]
    fn distinct_actors_have_distinct_counters_and_ids() {
        // DL-11: two offline actors each adding the FIRST field to the same
        // collection must get distinct ids.
        let a = actor("alice");
        let b = actor("bob");
        let mut c = CollectionDef::new("tasks");
        let id_a = c.add_field(&a, "title", FieldType::Text, false, false).field_id().to_string();
        let id_b = c.add_field(&b, "title2", FieldType::Text, false, false).field_id().to_string();
        assert_ne!(id_a, id_b, "distinct actors must mint distinct first-field ids (DL-11)");
        assert_eq!(id_a, "f_alice_0");
        assert_eq!(id_b, "f_bob_0");
        // Each actor's counter is independent.
        assert_eq!(c.next_seq_for(&a), 1);
        assert_eq!(c.next_seq_for(&b), 1);
    }

    #[test]
    fn new_required_field_starts_in_warning_mode() {
        let mut c = CollectionDef::new("tasks");
        let f = c.add_field(&actor("alice"), "title", FieldType::Text, false, true);
        assert!(f.required());
        assert!(!f.enforced(), "DL-12: required starts as warn, not enforce");
        assert!(!f.requires_value(), "warn-mode required must not hard-require");
    }

    #[test]
    fn requires_value_only_when_required_enforced_and_live() {
        let a = actor("alice");
        let mut c = CollectionDef::new("tasks");
        c.add_field(&a, "title", FieldType::Text, false, true);
        let id = c.fields()[0].field_id().to_string();

        assert!(!c.field(&id).unwrap().requires_value(), "warn mode does not require");
        c.field_mut(&id).unwrap().set_enforced(true);
        assert!(c.field(&id).unwrap().requires_value());
        c.field_mut(&id).unwrap().set_deprecated(true);
        assert!(!c.field(&id).unwrap().requires_value(), "deprecated field is never required");
    }

    #[test]
    fn field_lookup_by_id_and_name() {
        let mut c = CollectionDef::new("tasks");
        c.add_field(&actor("alice"), "title", FieldType::Text, false, false);
        assert!(c.field("f_alice_0").is_some());
        assert!(c.field("f_alice_9").is_none());
        assert!(c.has_field_name("title"));
        assert!(!c.has_field_name("nope"));
    }

    #[test]
    fn parse_field_id_handles_underscored_actors() {
        assert_eq!(parse_field_id("f_alice_0"), Some(("alice", 0)));
        assert_eq!(parse_field_id("f_dev_01_3"), Some(("dev_01", 3)));
        assert_eq!(parse_field_id("f0"), None, "legacy ids are not range-checked");
        assert_eq!(parse_field_id("f__0"), None, "empty actor is rejected");
        assert_eq!(parse_field_id("nope"), None);
    }

    #[test]
    fn validate_invariants_catches_future_id() {
        let a = actor("alice");
        let mut c = CollectionDef::new("tasks");
        c.add_field(&a, "title", FieldType::Text, false, false);
        assert!(c.validate_invariants().is_ok());
        // Forge an id ahead of the counter.
        c.field_mut("f_alice_0").unwrap().field_id = "f_alice_5".into();
        assert!(c.validate_invariants().is_err());
    }

    #[test]
    fn validate_invariants_catches_duplicate_field_name() {
        // review 022 P2: the additive path rejects duplicate display names, but
        // deserialization bypasses it — validate_invariants must re-catch it.
        let a = actor("alice");
        let mut c = CollectionDef::new("tasks");
        c.add_field(&a, "title", FieldType::Text, false, false);
        c.add_field(&a, "done", FieldType::Bool, false, false);
        assert!(c.validate_invariants().is_ok());
        // Tamper the second field to share the first field's display name.
        c.field_mut("f_alice_1").unwrap().name = "title".into();
        let err = c.validate_invariants().unwrap_err();
        assert!(err.contains("duplicate field name"), "got {err:?}");
    }
}
