//! Field and collection definitions.
//!
//! prd-merged/02 DL-7 (stable `field_id`, never reused; renames touch only the
//! display name), DL-8 (additive evolution), DL-12 (constraints warn before
//! they enforce).

use crate::field_type::FieldType;
use serde::{Deserialize, Serialize};

/// A single field in a collection's schema.
///
/// The `field_id` is **stable and never reused** (DL-7): a rename changes only
/// `name`. `deprecated` hides a field from new writes/UI without removing it
/// (DL-8 "deprecate = hide", "delete = deprecate + retain"), so old data keeps
/// round-tripping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    /// Stable identifier, e.g. `f0`, `f1`. Allocated once and never reused.
    pub field_id: String,
    /// Display name. The only thing a rename touches (DL-7).
    pub name: String,
    /// Value type; may only ever widen (DL-8, see [`FieldType::can_widen_to`]).
    pub ty: FieldType,
    /// Whether the projection should build an expression index (DL-5).
    pub indexed: bool,
    /// Hidden from new writes/UI but retained for read (DL-8).
    pub deprecated: bool,
    /// Whether a value is required on write (DL-12). Only *enforced* once
    /// `enforced` flips true; until then a missing value is a warning.
    pub required: bool,
    /// Whether the `required` constraint is in enforcement mode. New
    /// constraints start in warning mode (`false`) per DL-12 ("new constraints
    /// default to warning mode before enforcement mode").
    pub enforced: bool,
}

impl FieldDef {
    /// True if this field's `required` constraint should *error* (rather than
    /// merely warn) on a missing value: it must be both required and enforced,
    /// and not deprecated (a deprecated field is never required-for-write).
    pub fn requires_value(&self) -> bool {
        self.required && self.enforced && !self.deprecated
    }
}

/// A logical collection (≈ table): an ordered set of [`FieldDef`]s plus the
/// monotone sequence used to mint never-reused field ids (DL-7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// Next field sequence number. Field ids are `f{seq}`; this only ever
    /// increases, even across deprecations, so an id is never reused (DL-7).
    pub next_field_seq: u64,
}

impl CollectionDef {
    /// An empty collection named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        CollectionDef { name: name.into(), fields: Vec::new(), next_field_seq: 0 }
    }

    /// The id the next allocated field will receive (`f{next_field_seq}`),
    /// without consuming the sequence. Pure query — used by callers/tests.
    pub fn peek_next_field_id(&self) -> String {
        format!("f{}", self.next_field_seq)
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

    /// Allocate a new field, minting a fresh stable id from `next_field_seq`.
    ///
    /// `required` constraints always start in **warning mode** (`enforced =
    /// false`) per DL-12; the caller must explicitly enforce later.
    pub(crate) fn add_field(
        &mut self,
        name: impl Into<String>,
        ty: FieldType,
        indexed: bool,
        required: bool,
    ) -> &FieldDef {
        let field_id = self.peek_next_field_id();
        self.next_field_seq += 1;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_field_allocates_sequential_stable_ids() {
        let mut c = CollectionDef::new("tasks");
        assert_eq!(c.peek_next_field_id(), "f0");
        let id0 = c.add_field("title", FieldType::Text, false, false).field_id.clone();
        let id1 = c.add_field("done", FieldType::Bool, false, false).field_id.clone();
        assert_eq!(id0, "f0");
        assert_eq!(id1, "f1");
        assert_eq!(c.next_field_seq, 2);
        assert_eq!(c.peek_next_field_id(), "f2");
    }

    #[test]
    fn new_required_field_starts_in_warning_mode() {
        let mut c = CollectionDef::new("tasks");
        let f = c.add_field("title", FieldType::Text, false, true);
        assert!(f.required);
        assert!(!f.enforced, "DL-12: required starts as warn, not enforce");
        assert!(!f.requires_value(), "warn-mode required must not hard-require");
    }

    #[test]
    fn requires_value_only_when_required_enforced_and_live() {
        let f_warn = FieldDef {
            field_id: "f0".into(),
            name: "title".into(),
            ty: FieldType::Text,
            indexed: false,
            deprecated: false,
            required: true,
            enforced: false,
        };
        assert!(!f_warn.requires_value());

        let f_enforced = FieldDef { enforced: true, ..f_warn.clone() };
        assert!(f_enforced.requires_value());

        let f_deprecated = FieldDef { deprecated: true, ..f_enforced.clone() };
        assert!(!f_deprecated.requires_value(), "deprecated field is never required");

        let f_optional = FieldDef { required: false, ..f_enforced };
        assert!(!f_optional.requires_value());
    }

    #[test]
    fn field_lookup_by_id_and_name() {
        let mut c = CollectionDef::new("tasks");
        c.add_field("title", FieldType::Text, false, false);
        assert!(c.field("f0").is_some());
        assert!(c.field("f9").is_none());
        assert!(c.has_field_name("title"));
        assert!(!c.has_field_name("nope"));
    }
}
