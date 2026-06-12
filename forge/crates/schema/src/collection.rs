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

    /// Re-validate structural invariants after deserialization (review 005 P2):
    /// field ids must be unique within the collection and every field's id must
    /// be reachable by *some* actor counter (no id minted past its counter — an
    /// id `f_<actor>_<n>` requires `next_field_seq[actor] > n`), so a
    /// hand-built registry can't smuggle in a future-colliding id.
    pub(crate) fn validate_invariants(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.fields {
            if !seen.insert(f.field_id.as_str()) {
                return Err(format!("duplicate field id {:?} in collection {:?}", f.field_id, self.name));
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
}
