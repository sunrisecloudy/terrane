//! Dynamic schema registry with additive-only evolution.
//!
//! prd-merged/02 DL-7 (stable field ids, never reused; **per-actor id ranges:
//! actor-id ⊕ counter**), DL-8 (additive-only: add collection/field, widen
//! type, deprecate; destructive ops are *not* exposed), DL-9/DL-10
//! (unknown-collection/field tolerance), DL-11 (registry versions are CRDT
//! vectors — two offline actors adding different fields merge to the union *by
//! construction*, which is why field ids are actor-scoped), DL-12 (constraints
//! warn before they enforce).
//!
//! The registry is the authority on "what the schema is". It is pure logic
//! (no I/O, `wasm32`-clean): the storage layer persists it as the
//! `schema_registry_doc` CRDT (DL-2), but the rules live here.
//!
//! Internals (`collections`, `CollectionDef.fields`, the per-actor counters,
//! and `FieldDef`'s flags) are private (review 005 P2): external crates read
//! through accessors and mutate only via [`SchemaChange`], so they cannot
//! bypass the additive-only / id-stability invariants. A deserialized registry
//! is re-validated via [`SchemaRegistry::validated`].

use crate::collection::CollectionDef;
use crate::field_type::FieldType;
use forge_domain::{ActorId, CoreError, RecordEnvelope, Result};
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
    /// Add a field to an existing collection; mints a fresh **actor-scoped**
    /// stable id `f_<actor>_<seq>` (DL-7/DL-11) so two offline actors adding the
    /// first field to the same collection get distinct ids. A `required` field
    /// is added in warning mode (DL-12).
    AddField {
        collection: String,
        /// The actor minting this field. Its id range is `actor ⊕ counter`.
        actor: ActorId,
        name: String,
        ty: FieldType,
        #[serde(default)]
        indexed: bool,
        #[serde(default)]
        required: bool,
    },
    /// Rename a field's display name (DL-7): the stable id is untouched.
    RenameField { collection: String, field_id: String, name: String },
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
/// Because field ids are actor-scoped, an offline merge of two registries is
/// collision-free by construction.
///
/// `collections` is private (review 005 P2): callers mutate only via
/// [`SchemaChange`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaRegistry {
    collections: BTreeMap<String, CollectionDef>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        SchemaRegistry::default()
    }

    /// Construct from a (possibly externally produced / deserialized) registry,
    /// re-validating structural invariants (review 005 P2). Use this instead of
    /// `serde_json::from_*` directly when ingesting untrusted state so a
    /// hand-built registry can't smuggle in a colliding/future field id.
    pub fn validated(self) -> Result<Self> {
        for col in self.collections.values() {
            col.validate_invariants().map_err(CoreError::SchemaCompatibilityError)?;
        }
        Ok(self)
    }

    // ----------------------------------------------------------------- queries

    /// Read-only iterator over (name, collection) pairs.
    pub fn collections(&self) -> impl Iterator<Item = (&str, &CollectionDef)> {
        self.collections.iter().map(|(k, v)| (k.as_str(), v))
    }

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
            SchemaChange::AddField { collection, actor, name, ty, indexed, required } => {
                self.add_field(&collection, &actor, &name, ty, indexed, required)
            }
            SchemaChange::RenameField { collection, field_id, name } => {
                self.rename_field(&collection, &field_id, &name)
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
        actor: &ActorId,
        name: &str,
        ty: FieldType,
        indexed: bool,
        required: bool,
    ) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::ValidationError("field name is empty".into()));
        }
        if actor.as_str().trim().is_empty() {
            return Err(CoreError::ValidationError("actor id is empty".into()));
        }
        let col = self.require_collection_mut(collection)?;
        if col.has_field_name(name) {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "field name {name:?} already exists in collection {collection:?}"
            )));
        }
        col.add_field(actor, name, ty, indexed, required);
        Ok(())
    }

    fn rename_field(&mut self, collection: &str, field_id: &str, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::ValidationError("field name is empty".into()));
        }
        let col = self.require_collection_mut(collection)?;
        // Reject a rename that would collide with another field's display name.
        if col.field(field_id).map(|f| f.name()) != Some(name) && col.has_field_name(name) {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "field name {name:?} already exists in collection {collection:?}"
            )));
        }
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        field.rename(name); // DL-7: id untouched.
        Ok(())
    }

    fn widen_field(&mut self, collection: &str, field_id: &str, to: FieldType) -> Result<()> {
        let col = self.require_collection_mut(collection)?;
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        if *field.ty() == to {
            return Ok(()); // idempotent no-op widen.
        }
        if !field.ty().can_widen_to(&to) {
            return Err(CoreError::SchemaCompatibilityError(format!(
                "cannot widen field {field_id:?} from {:?} to {to:?}: not an additive widening",
                field.ty()
            )));
        }
        field.set_type(to);
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
        field.set_deprecated(true);
        Ok(())
    }

    fn enforce_required(&mut self, collection: &str, field_id: &str) -> Result<()> {
        let col = self.require_collection_mut(collection)?;
        let field = col.field_mut(field_id).ok_or_else(|| {
            CoreError::SchemaCompatibilityError(format!(
                "unknown field {field_id:?} in collection {collection:?}"
            ))
        })?;
        if !field.required() {
            return Err(CoreError::ValidationError(format!(
                "field {field_id:?} is not a required field; nothing to enforce"
            )));
        }
        field.set_enforced(true);
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
    /// Validation is **by stable `field_id`** (DL-7): `record.field_ids` is the
    /// authoritative carrier (DL §5), so required/unknown checks key off it, and
    /// display names are only a projection. This means a renamed field's old
    /// record — which still carries `field_ids["f_..."]` even though its display
    /// name changed — is *not* flagged missing. The display-name map is consulted
    /// only as a fallback for records that predate `field_ids` population.
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

        // DL-7: required/unknown checks key off the authoritative stable ids. A
        // value "is present" if either the stable id is in `field_ids`, OR
        // (fallback for pre-`field_ids` records) the display name is in `fields`.
        for field in col.fields() {
            if field.deprecated() {
                continue; // deprecated fields impose no write constraints (DL-8).
            }
            let present = record.field_ids.contains_key(field.field_id())
                || record.fields.contains_key(field.name());
            if !present && field.required() {
                if field.enforced() {
                    // DL-12 enforcement mode: this is now a hard error.
                    return Err(CoreError::ValidationError(format!(
                        "required field {:?} (id {}) missing in collection {collection:?}",
                        field.name(),
                        field.field_id()
                    )));
                }
                // DL-12 warning mode: warn but accept.
                warnings.push(SchemaWarning::field(
                    field.field_id(),
                    format!(
                        "field {:?} is required (warn mode) but missing; will become an \
                         error once enforced",
                        field.name()
                    ),
                ));
            }
        }

        // DL-9: any stable id the schema does not know is preserved verbatim
        // with a capability-style warning rather than a rejection. This is the
        // authoritative check (a renamed field's old id stays known here).
        for field_id in record.field_ids.keys() {
            if col.field(field_id).is_none() {
                warnings.push(SchemaWarning::record(format!(
                    "field id {field_id:?} is unknown in collection {collection:?}; \
                     preserved (DL-9)"
                )));
            }
        }

        // DL-9 (projection fallback): a *display* field that maps to no known id
        // and isn't already covered by a stable id above. Only display names the
        // schema doesn't know surface here; a renamed field whose record carries
        // the stable id is NOT re-flagged.
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
    /// - an *un-deprecation* (deprecated → live), which changes the constraint
    ///   surface retroactively;
    /// - any actor's per-actor `next_field_seq` counter went **backwards**,
    ///   which would risk minting a previously-used id (DL-7/DL-11).
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
        // DL-7/DL-11: no actor's id counter may go backwards (would reuse ids).
        for (actor, old_seq) in old_col.actor_seqs() {
            let new_seq = new_col.actor_seqs().get(actor).copied().unwrap_or(0);
            if new_seq < *old_seq {
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: actor {actor:?} next_field_seq went backwards \
                     ({old_seq} -> {new_seq}); field ids would be reused (DL-7)"
                )));
            }
        }

        for old_field in old_col.fields() {
            let new_field = new_col.field(old_field.field_id()).ok_or_else(|| {
                CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field id {:?} was removed; removal is not additive \
                     (use deprecate; DL-8)",
                    old_field.field_id()
                ))
            })?;

            // DL-8: the type may only have widened (or stayed identical).
            if !old_field.ty().can_widen_to(new_field.ty()) {
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field {} type narrowed from {:?} to {:?} (DL-8)",
                    old_field.field_id(),
                    old_field.ty(),
                    new_field.ty()
                )));
            }

            // DL-8: deprecate is one-way (hide + retain). Un-deprecating would
            // resurrect a hidden field and is not an additive change.
            if old_field.deprecated() && !new_field.deprecated() {
                return Err(CoreError::SchemaCompatibilityError(format!(
                    "collection {name:?}: field {} was un-deprecated; deprecation is one-way \
                     (DL-8)",
                    old_field.field_id()
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

    fn actor(s: &str) -> ActorId {
        ActorId::new(s)
    }

    /// Build a record with display `fields` and stable `field_ids`.
    fn rec_full(
        collection: &str,
        fields: &[(&str, serde_json::Value)],
        field_ids: &[(&str, serde_json::Value)],
    ) -> RecordEnvelope {
        let mut r = RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new("rec_1"),
            fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<Map<_, _>>(),
            LogicalTimestamp(1),
        );
        r.field_ids = field_ids.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        r
    }

    fn rec(collection: &str, fields: &[(&str, serde_json::Value)]) -> RecordEnvelope {
        rec_full(collection, fields, &[])
    }

    /// Helper: a registry with a `tasks` collection holding `title` (text) and
    /// `done` (bool), both minted by actor `alice` (ids `f_alice_0`/`f_alice_1`).
    fn tasks_registry() -> SchemaRegistry {
        let a = actor("alice");
        let mut r = SchemaRegistry::new();
        r.apply_change(SchemaChange::AddCollection { name: "tasks".into() }).unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: a.clone(),
            name: "title".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: a,
            name: "done".into(),
            ty: FieldType::Bool,
            indexed: false,
            required: false,
        })
        .unwrap();
        r
    }

    // -------- DL-7/DL-11: stable, actor-scoped, never-reused field ids --------

    #[test]
    fn add_collection_then_fields_allocate_actor_scoped_stable_ids() {
        let r = tasks_registry();
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.fields()[0].field_id(), "f_alice_0");
        assert_eq!(col.fields()[1].field_id(), "f_alice_1");
        assert_eq!(col.next_seq_for(&actor("alice")), 2);
    }

    #[test]
    fn two_offline_actors_first_field_get_distinct_ids() {
        // DL-11: the headline guarantee. Start from a shared base collection,
        // then have two actors *each* add a field offline; their ids differ so
        // the offline merge is collision-free by construction.
        let mut base = SchemaRegistry::new();
        base.apply_change(SchemaChange::AddCollection { name: "tasks".into() }).unwrap();

        let mut alice = base.clone();
        alice
            .apply_change(SchemaChange::AddField {
                collection: "tasks".into(),
                actor: actor("alice"),
                name: "title".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap();

        let mut bob = base.clone();
        bob.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("bob"),
            name: "title".into(), // same DISPLAY name, different actor
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();

        let id_a = alice.collection("tasks").unwrap().fields()[0].field_id();
        let id_b = bob.collection("tasks").unwrap().fields()[0].field_id();
        assert_eq!(id_a, "f_alice_0");
        assert_eq!(id_b, "f_bob_0");
        assert_ne!(id_a, id_b, "distinct actors must mint distinct first-field ids (DL-11)");
    }

    #[test]
    fn renaming_a_field_keeps_its_id() {
        // A rename via the SchemaChange API: only the display name changes.
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::RenameField {
            collection: "tasks".into(),
            field_id: "f_alice_0".into(),
            name: "label".into(),
        })
        .unwrap();
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.field("f_alice_0").unwrap().name(), "label");
        assert_eq!(col.field("f_alice_0").unwrap().field_id(), "f_alice_0", "rename must not touch id");
        // And it is forward-compatible: a rename is purely additive.
        let old = tasks_registry();
        assert!(r.validate_compatibility(&old).is_ok());
    }

    #[test]
    fn rename_to_duplicate_name_is_rejected() {
        let mut r = tasks_registry();
        let err = r
            .apply_change(SchemaChange::RenameField {
                collection: "tasks".into(),
                field_id: "f_alice_0".into(),
                name: "done".into(), // already used by f_alice_1
            })
            .unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn rename_to_same_name_is_noop_ok() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::RenameField {
            collection: "tasks".into(),
            field_id: "f_alice_0".into(),
            name: "title".into(),
        })
        .unwrap();
        assert_eq!(r.collection("tasks").unwrap().field("f_alice_0").unwrap().name(), "title");
    }

    #[test]
    fn deprecating_then_adding_does_not_reuse_ids() {
        let a = actor("alice");
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f_alice_0".into(),
        })
        .unwrap();
        // New field gets f_alice_2, NOT the freed-looking f_alice_0 (DL-7).
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: a,
            name: "title_v2".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();
        let col = r.collection("tasks").unwrap();
        assert_eq!(col.field("f_alice_2").unwrap().name(), "title_v2");
        assert!(col.field("f_alice_0").unwrap().deprecated(), "old id still present, hidden");
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
                actor: actor("alice"),
                name: "title".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn empty_actor_id_is_validation_error() {
        let mut r = SchemaRegistry::new();
        r.apply_change(SchemaChange::AddCollection { name: "c".into() }).unwrap();
        let err = r
            .apply_change(SchemaChange::AddField {
                collection: "c".into(),
                actor: actor("  "),
                name: "x".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn widen_int_to_float_ok_float_to_int_rejected() {
        let mut r = SchemaRegistry::new();
        r.apply_change(SchemaChange::AddCollection { name: "m".into() }).unwrap();
        r.apply_change(SchemaChange::AddField {
            collection: "m".into(),
            actor: actor("alice"),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: false,
            required: false,
        })
        .unwrap();
        // Int -> Float OK.
        r.apply_change(SchemaChange::WidenField {
            collection: "m".into(),
            field_id: "f_alice_0".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
        assert_eq!(*r.collection("m").unwrap().field("f_alice_0").unwrap().ty(), FieldType::FloatNum);
        // Float -> Int rejected.
        let err = r
            .apply_change(SchemaChange::WidenField {
                collection: "m".into(),
                field_id: "f_alice_0".into(),
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
            field_id: "f_alice_0".into(),
            to: FieldType::Text,
        })
        .unwrap();
        assert_eq!(*r.collection("tasks").unwrap().field("f_alice_0").unwrap().ty(), FieldType::Text);
    }

    #[test]
    fn deprecate_hides_but_field_still_listed() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f_alice_1".into(),
        })
        .unwrap();
        let col = r.collection("tasks").unwrap();
        assert!(col.field("f_alice_1").unwrap().deprecated());
        // Still listed (retained), not removed (DL-8 "delete = deprecate + retain").
        assert_eq!(col.fields().len(), 2);
    }

    #[test]
    fn widen_unknown_field_or_collection_errors() {
        let mut r = tasks_registry();
        assert_eq!(
            r.apply_change(SchemaChange::WidenField {
                collection: "tasks".into(),
                field_id: "f_alice_99".into(),
                to: FieldType::Scalar,
            })
            .unwrap_err()
            .code(),
            "SchemaCompatibilityError"
        );
        assert_eq!(
            r.apply_change(SchemaChange::AddField {
                collection: "ghost".into(),
                actor: actor("alice"),
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
                actor: actor("alice"),
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
    // These exercise the invariant by *deserializing* a hand-tampered registry
    // (internals are private — review 005 P2 — so we can no longer poke fields
    // directly from an integration caller).

    fn tampered(json: serde_json::Value) -> SchemaRegistry {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn removing_a_field_is_rejected_by_compatibility() {
        let old = tasks_registry();
        // Diverged registry that dropped f_alice_1.
        let mut json = serde_json::to_value(&old).unwrap();
        let fields = json["collections"]["tasks"]["fields"].as_array_mut().unwrap();
        fields.retain(|f| f["field_id"] != serde_json::json!("f_alice_1"));
        let diverged = tampered(json);
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        assert!(err.to_string().contains("f_alice_1"));
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
            actor: actor("alice"),
            name: "amount".into(),
            ty: FieldType::FloatNum,
            indexed: false,
            required: false,
        })
        .unwrap();
        // Diverged: narrowed Float -> Int.
        let mut json = serde_json::to_value(&old).unwrap();
        json["collections"]["m"]["fields"][0]["ty"] = serde_json::json!("int_num");
        let diverged = tampered(json);
        assert_eq!(
            diverged.validate_compatibility(&old).unwrap_err().code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn reusing_a_field_id_is_rejected_by_compatibility() {
        let old = tasks_registry();
        // Diverged: f_alice_1 ("done", Bool) re-typed to an incompatible type
        // while keeping the same id — a reuse of the id (DL-7).
        let mut json = serde_json::to_value(&old).unwrap();
        let f = &mut json["collections"]["tasks"]["fields"][1];
        f["name"] = serde_json::json!("priority");
        f["ty"] = serde_json::json!("text"); // Bool cannot widen to Text.
        let diverged = tampered(json);
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
    }

    #[test]
    fn seq_going_backwards_is_rejected_by_compatibility() {
        let old = tasks_registry();
        let mut json = serde_json::to_value(&old).unwrap();
        json["collections"]["tasks"]["next_field_seq"]["alice"] = serde_json::json!(1);
        let diverged = tampered(json);
        let err = diverged.validate_compatibility(&old).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        assert!(err.to_string().contains("reused"));
    }

    #[test]
    fn un_deprecating_a_field_is_rejected_by_compatibility() {
        let mut old = tasks_registry();
        old.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f_alice_0".into(),
        })
        .unwrap();
        let mut json = serde_json::to_value(&old).unwrap();
        json["collections"]["tasks"]["fields"][0]["deprecated"] = serde_json::json!(false);
        let diverged = tampered(json);
        assert_eq!(
            diverged.validate_compatibility(&old).unwrap_err().code(),
            "SchemaCompatibilityError"
        );
    }

    #[test]
    fn additive_evolution_is_compatible() {
        let old = tasks_registry();
        let mut new = old.clone();
        // Add a field, widen it, add a new collection, deprecate a field — all
        // additive.
        new.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "priority".into(),
            ty: FieldType::IntNum,
            indexed: true,
            required: false,
        })
        .unwrap();
        new.apply_change(SchemaChange::WidenField {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
        new.apply_change(SchemaChange::AddCollection { name: "notes".into() }).unwrap();
        new.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f_alice_1".into(),
        })
        .unwrap();
        assert!(new.validate_compatibility(&old).is_ok());
    }

    #[test]
    fn merging_two_actors_fields_is_mutually_compatible() {
        // DL-11: the offline-merge invariant. From a shared base, alice and bob
        // each add a field; the union of both is a valid additive evolution of
        // either branch (no id collision, no counter rollback).
        let mut base = SchemaRegistry::new();
        base.apply_change(SchemaChange::AddCollection { name: "tasks".into() }).unwrap();
        let mut alice = base.clone();
        alice
            .apply_change(SchemaChange::AddField {
                collection: "tasks".into(),
                actor: actor("alice"),
                name: "title".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap();
        // Union = alice's field PLUS bob's field, applied on top of alice.
        let mut union = alice.clone();
        union
            .apply_change(SchemaChange::AddField {
                collection: "tasks".into(),
                actor: actor("bob"),
                name: "title2".into(),
                ty: FieldType::Text,
                indexed: false,
                required: false,
            })
            .unwrap();
        assert!(union.validate_compatibility(&alice).is_ok());
        assert!(union.validate_compatibility(&base).is_ok());
        // Distinct ids present.
        let col = union.collection("tasks").unwrap();
        assert!(col.field("f_alice_0").is_some());
        assert!(col.field("f_bob_0").is_some());
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
        assert!(r.is_known_field("tasks", "f_alice_0"));
        assert!(!r.is_known_field("tasks", "f_alice_99"));
        assert!(!r.is_known_field("ghost", "f_alice_0"));
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

    #[test]
    fn unknown_stable_id_surfaces_as_dl9_warning() {
        // DL-9 via the authoritative path: a stable id the schema doesn't know.
        let r = tasks_registry();
        let record = rec_full(
            "tasks",
            &[],
            &[("f_alice_0", serde_json::json!("hi")), ("f_future_3", serde_json::json!(1))],
        );
        let warnings = r.validate_record("tasks", &record).unwrap();
        assert!(
            warnings.iter().any(|w| w.message.contains("f_future_3") && w.message.contains("DL-9")),
            "unknown stable id must surface as a DL-9 warning, got {warnings:?}"
        );
    }

    // -------- DL-7: validate by field_id, rename keeps old records valid ------

    #[test]
    fn renamed_field_old_record_is_not_flagged_missing_or_unknown() {
        // The headline DL-7 fix (review 005 P1): make `title` required+enforced,
        // then RENAME it to `label`. A record that still carries the OLD shape —
        // i.e. only the stable id `f_alice_0` in `field_ids` — must validate
        // cleanly: not "missing required" (id is present) and not "unknown"
        // (the id is still known).
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
        })
        .unwrap();
        r.apply_change(SchemaChange::RenameField {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
            name: "assignee".into(), // renamed AFTER the record was written
        })
        .unwrap();

        // Record carries only stable ids, including the OLD field's id.
        let record = rec_full(
            "tasks",
            &[],
            &[("f_alice_0", serde_json::json!("hi")), ("f_alice_2", serde_json::json!("me"))],
        );
        let warnings = r
            .validate_record("tasks", &record)
            .expect("renamed required field present by id must not hard-fail (DL-7)");
        assert!(
            !warnings.iter().any(|w| w.field_id.as_deref() == Some("f_alice_2")),
            "renamed field present by stable id must not be flagged missing: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.message.contains("unknown")),
            "stable ids are all known; no DL-9 unknown warning expected: {warnings:?}"
        );
    }

    #[test]
    fn enforced_required_missing_by_id_hard_fails() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
        })
        .unwrap();
        // A record that carries other ids but NOT f_alice_2 hard-fails.
        let record = rec_full("tasks", &[], &[("f_alice_0", serde_json::json!("hi"))]);
        let err = r.validate_record("tasks", &record).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    // -------- DL-12: warn-before-enforce --------

    #[test]
    fn required_constraint_starts_warn_then_enforces() {
        let mut r = tasks_registry();
        // Add a required field (starts in warn mode per DL-12).
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        let owner = r.collection("tasks").unwrap().field("f_alice_2").unwrap();
        assert_eq!(owner.field_id(), "f_alice_2");
        assert!(owner.required());
        assert!(!owner.enforced());

        // Record missing `owner`: warn mode => Ok with a warning, NOT an error.
        let record = rec("tasks", &[("title", serde_json::json!("hi"))]);
        let warnings = r.validate_record("tasks", &record).unwrap();
        assert!(
            warnings.iter().any(|w| w.field_id.as_deref() == Some("f_alice_2")
                && w.message.contains("warn mode")),
            "warn-mode required must surface a warning, got {warnings:?}"
        );

        // Graduate to enforcement.
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
        })
        .unwrap();
        assert!(r.collection("tasks").unwrap().field("f_alice_2").unwrap().enforced());

        // Now the same missing-value record HARD-fails.
        let err = r.validate_record("tasks", &record).unwrap_err();
        assert_eq!(err.code(), "ValidationError");

        // ...but a record that supplies `owner` (by display name) passes cleanly.
        let ok_record =
            rec("tasks", &[("title", serde_json::json!("hi")), ("owner", serde_json::json!("me"))]);
        assert!(r.validate_record("tasks", &ok_record).unwrap().is_empty());
    }

    #[test]
    fn enforce_on_non_required_field_is_validation_error() {
        let mut r = tasks_registry();
        // f_alice_0 (title) is not required.
        let err = r
            .apply_change(SchemaChange::EnforceRequired {
                collection: "tasks".into(),
                field_id: "f_alice_0".into(),
            })
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn enforced_required_but_deprecated_field_does_not_block() {
        let mut r = tasks_registry();
        r.apply_change(SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "owner".into(),
            ty: FieldType::Text,
            indexed: false,
            required: true,
        })
        .unwrap();
        r.apply_change(SchemaChange::EnforceRequired {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
        })
        .unwrap();
        // Then deprecate it: a deprecated field imposes no write constraint.
        r.apply_change(SchemaChange::DeprecateField {
            collection: "tasks".into(),
            field_id: "f_alice_2".into(),
        })
        .unwrap();
        let record = rec("tasks", &[("title", serde_json::json!("hi"))]);
        assert!(
            r.validate_record("tasks", &record).is_ok(),
            "deprecated required field must not hard-block writes"
        );
    }

    // -------- review 005 P2: encapsulation / validate-on-deserialize ----------

    #[test]
    fn validated_rejects_future_field_id() {
        let good = tasks_registry();
        assert!(good.clone().validated().is_ok());
        // Tamper: bump a field id ahead of its actor counter.
        let mut json = serde_json::to_value(&good).unwrap();
        json["collections"]["tasks"]["fields"][0]["field_id"] = serde_json::json!("f_alice_9");
        let bad: SchemaRegistry = serde_json::from_value(json).unwrap();
        assert_eq!(bad.validated().unwrap_err().code(), "SchemaCompatibilityError");
    }

    #[test]
    fn validated_rejects_duplicate_field_id() {
        let good = tasks_registry();
        let mut json = serde_json::to_value(&good).unwrap();
        // Make the second field a duplicate id of the first.
        json["collections"]["tasks"]["fields"][1]["field_id"] = serde_json::json!("f_alice_0");
        let bad: SchemaRegistry = serde_json::from_value(json).unwrap();
        assert_eq!(bad.validated().unwrap_err().code(), "SchemaCompatibilityError");
    }

    #[test]
    fn change_roundtrips_json() {
        let c = SchemaChange::AddField {
            collection: "tasks".into(),
            actor: actor("alice"),
            name: "amount".into(),
            ty: FieldType::nullable(FieldType::IntNum),
            indexed: true,
            required: true,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"op\":\"add_field\""), "{s}");
        assert!(s.contains("\"actor\":\"alice\""), "{s}");
        let back: SchemaChange = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn registry_roundtrips_json() {
        let r = tasks_registry();
        let s = serde_json::to_string(&r).unwrap();
        let back: SchemaRegistry = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
        assert!(back.validated().is_ok());
    }
}
