//! `schema.apply_change` / `schema.validate_compatibility` / `schema.rebuild_indexes`
//! — the dynamic-schema commands (DL-7/DL-8 → DL-5). Moved verbatim from
//! `workspace.rs` (/simplify #11a): the three handlers plus the registry/index
//! helpers they (and the open-time index reconstruction in `workspace.rs`) share.

use forge_domain::{CoreError, Result};
use forge_schema::{CollectionDef, FieldDef, FieldType, SchemaChange, SchemaRegistry};
use forge_storage::{CreateIndexKind, IndexDef, IndexState};

use super::super::persistence::META_NS;
use super::super::{WorkspaceCore, SCHEMA_REGISTRY_KEY};
use super::take_field;

impl WorkspaceCore {
    /// `schema.apply_change` — apply one additive [`SchemaChange`] to the dynamic
    /// registry, persist the new registry, and return the new collection/registry
    /// summary (CR-A2; `forge/spec/commands.md`: Owner/Maintainer, DL-8).
    ///
    /// Payload: `{ change }` — a serialized [`SchemaChange`] (the `op`-tagged
    /// snake_case shape the schema crate defines). The schema crate is the
    /// authority: it mints stable actor-scoped field ids (DL-7), enforces
    /// additive-only evolution, and **rejects** a destructive/incompatible change
    /// with [`CoreError::SchemaCompatibilityError`] (e.g. re-adding a collection,
    /// duplicate field name, narrowing a type) — we surface that verbatim and the
    /// registry is left unchanged (we only persist on success).
    ///
    /// DL-8 → DL-5: when an `add_field` marks the field `indexed`, we CREATE the
    /// corresponding storage index over the field's freshly minted **stable**
    /// `field_id` ([`Store::create_index`]) so the dynamic index follows the
    /// schema. A `Text` field gets an FTS5 shadow table; any other type gets a
    /// JSON1 expression (`Value`) index.
    pub(in crate::workspace) fn cmd_schema_apply_change(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let change: SchemaChange = take_field(cmd, "change")?;

        // Apply to a COPY first so a rejected change leaves the live registry (and
        // therefore the persisted state) untouched. The schema crate returns the
        // SchemaCompatibilityError for destructive/incompatible changes.
        let mut next = self.registry.clone();
        next.apply_change(change.clone())?;

        // DL-8 → DL-5: a newly added `indexed` field gets its storage index built
        // over the stable field id the schema crate just minted.
        //
        // Create the index BEFORE persisting/swapping the registry (review 066): a
        // schema-minted field id interpolates the actor id (`f_<actor>_<seq>`), and
        // an actor id with characters outside the storage identifier charset (e.g.
        // `alice@example.com`) makes `create_index` fail. If we persisted first, the
        // rejected change would still be on disk and `rebuild_indexes_from_registry`
        // would fail on EVERY future open — poisoning the workspace. By creating the
        // index against the candidate registry first, an invalid field id rejects the
        // whole `apply_change` (`QueryError`) with the live + persisted registry
        // untouched.
        let mut created_index: Option<String> = None;
        if let SchemaChange::AddField { collection, indexed: true, .. } = &change {
            if let Some((field_id, kind)) = indexed_field_to_create(&next, collection) {
                let id = self.store.create_index(
                    &mut self.indexes,
                    collection,
                    &field_id,
                    kind,
                )?;
                created_index = Some(id);
            }
        }

        // The index (if any) was created successfully — now durably commit the
        // evolved registry. Persist BEFORE swapping the in-memory copy so the
        // durable schema and the in-memory one never diverge.
        self.persist_registry(&next)?;
        self.registry = next;

        self.events.emit(
            None,
            "schema.changed",
            serde_json::json!({ "workspace_id": self.workspace_id, "op": change_op(&change) }),
        );

        Ok(serde_json::json!({
            "op": change_op(&change),
            "registry": registry_summary(&self.registry),
            "created_index": created_index,
        }))
    }

    /// `schema.validate_compatibility` — prove the CURRENT registry is a
    /// forward-compatible, additive-only evolution of a baseline (CR-A2;
    /// `forge/spec/commands.md`: Owner/Maintainer/Editor/Auditor, DL-8).
    ///
    /// Payload: `{ against? }` — an optional baseline [`SchemaRegistry`] (the
    /// serialized form) the current registry must be a forward evolution of. When
    /// omitted the baseline is the empty registry (every registry is trivially a
    /// compatible evolution of empty), so the command doubles as a structural
    /// self-check. The supplied baseline is re-validated
    /// ([`SchemaRegistry::validated`]) so a hand-built/tampered baseline can't
    /// smuggle in a future-colliding id.
    ///
    /// Returns `{ ok, warnings }`. `ok: false` carries the
    /// [`CoreError::SchemaCompatibilityError`] message as the single warning rather
    /// than failing the command, so a UI can show the incompatibility without the
    /// request itself erroring (the destructive *apply* path is the one that hard-
    /// rejects).
    pub(in crate::workspace) fn cmd_schema_validate_compatibility(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let baseline = match cmd.payload.get("against") {
            None | Some(serde_json::Value::Null) => SchemaRegistry::new(),
            Some(v) => {
                let parsed: SchemaRegistry = serde_json::from_value(v.clone()).map_err(|e| {
                    CoreError::ValidationError(format!(
                        "schema.validate_compatibility `against` is malformed: {e}"
                    ))
                })?;
                // Re-validate the untrusted baseline before comparing against it.
                parsed.validated()?
            }
        };
        match self.registry.validate_compatibility(&baseline) {
            Ok(()) => Ok(serde_json::json!({ "ok": true, "warnings": [] })),
            Err(e) => Ok(serde_json::json!({ "ok": false, "warnings": [e.to_string()] })),
        }
    }

    /// `schema.rebuild_indexes` — rebuild the storage indexes for the registry's
    /// `indexed` fields purely from canonical `records` (CR-A2;
    /// `forge/spec/commands.md`: Owner/Maintainer, DL-5/DL-6).
    ///
    /// Payload: `{ collection?, index_ids? }` — optional filters that narrow the
    /// rebuild to one collection and/or a set of index ids; absent → rebuild every
    /// registered index. The registry is the source of truth for *which* fields
    /// are indexed, so we first (re)register a definition for each `indexed` field
    /// (DL-8 → DL-5), then drop+recreate each selected physical structure from
    /// canonical records via [`Store::build_indexes`] (DL-6 rebuild-source-of-
    /// truth: never reads prior index pages / FTS rows).
    pub(in crate::workspace) fn cmd_schema_rebuild_indexes(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let collection_filter = match cmd.payload.get("collection") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(other) => {
                return Err(CoreError::ValidationError(format!(
                    "schema.rebuild_indexes `collection` must be a string, got {other}"
                )))
            }
        };
        let index_id_filter = parse_index_ids(cmd)?;

        // Re-register a definition for every `indexed` field so the manager
        // reflects the current registry (idempotent: create_index replaces same-
        // kind defs). Building from canonical records means the physical structure
        // is correct even if records predate the index (DL-6).
        let rebuilt = self.rebuild_registry_indexes(
            collection_filter.as_deref(),
            index_id_filter.as_deref(),
        )?;

        Ok(serde_json::json!({
            "rebuilt": rebuilt,
            "rebuilt_count": rebuilt.len(),
        }))
    }

    /// (Re)build the storage indexes the registry declares (its `indexed` fields),
    /// optionally narrowed to one `collection` and/or a set of `index_ids`.
    /// Returns the ids of the indexes that were (re)built, in stable order. Each
    /// index is created from canonical records (DL-6), so a field indexed after
    /// rows already exist is populated correctly.
    fn rebuild_registry_indexes(
        &mut self,
        collection_filter: Option<&str>,
        index_id_filter: Option<&[String]>,
    ) -> Result<Vec<String>> {
        let mut rebuilt = Vec::new();
        for (collection, field_id, kind) in indexed_fields(&self.registry) {
            if let Some(want) = collection_filter {
                if collection != want {
                    continue;
                }
            }
            // Compute the deterministic index id for the id_filter check WITHOUT
            // creating it first (so a filtered-out index is never built). The
            // public IndexDef constructor derives the same canonical name
            // Store::create_index will use, and also validates the identifiers.
            if let Some(ids) = index_id_filter {
                let index_id =
                    IndexDef::new(collection.clone(), field_id.clone(), kind.into(), IndexState::Active)?
                        .index_id;
                if !ids.iter().any(|w| w == &index_id) {
                    continue;
                }
            }
            // create_index drops + recreates the physical structure from canonical
            // records and (re)registers the Active definition — the DL-6 rebuild.
            let id = self.store.create_index(&mut self.indexes, &collection, &field_id, kind)?;
            rebuilt.push(id);
        }
        Ok(rebuilt)
    }

    /// Persist the registry to the workspace file (`__forge/meta` /
    /// `schema_registry`) as serialized JSON, mirroring the `db.read` grant
    /// persistence. So a defined schema survives reopen (DL-7/DL-8).
    fn persist_registry(&self, registry: &SchemaRegistry) -> Result<()> {
        let bytes = serde_json::to_vec(registry)
            .map_err(|e| CoreError::StorageError(format!("serialize schema registry: {e}")))?;
        self.store
            .kv_set(META_NS, SCHEMA_REGISTRY_KEY, &bytes, "application/json")
    }
}

/// Every `(collection, field_id, kind)` the registry declares as `indexed`,
/// skipping deprecated fields (a hidden field's index is not maintained). The
/// kind is derived from the field type ([`index_kind_for`]). Stable iteration
/// order (registry collections are a `BTreeMap`, fields are declaration-ordered).
pub(in crate::workspace) fn indexed_fields(
    registry: &SchemaRegistry,
) -> Vec<(String, String, CreateIndexKind)> {
    let mut out = Vec::new();
    for (name, col) in registry.collections() {
        out.extend(collection_indexed_fields(name, col));
    }
    out
}

/// The `indexed` (non-deprecated) fields of one collection as
/// `(collection, field_id, kind)`.
fn collection_indexed_fields(
    name: &str,
    col: &CollectionDef,
) -> Vec<(String, String, CreateIndexKind)> {
    col.fields()
        .iter()
        .filter(|f| f.indexed() && !f.deprecated())
        .map(|f| (name.to_string(), f.field_id().to_string(), index_kind_for(f)))
        .collect()
}

/// The dynamic-index kind for a field (DL-5): a `Text` field gets a full-text
/// (`Fts`) shadow table; every other type gets an equality/range/order (`Value`)
/// expression index. The nullable wrapper is peeled so `Nullable(Text)` is still
/// full-text.
fn index_kind_for(field: &FieldDef) -> CreateIndexKind {
    match field.ty().inner() {
        FieldType::Text => CreateIndexKind::Fts,
        _ => CreateIndexKind::Value,
    }
}

/// The (stable field id, index kind) for the last-added field of `collection` in
/// `registry`, iff that field is marked `indexed` (DL-8 → DL-5). Takes the
/// registry explicitly so `schema.apply_change` can probe the CANDIDATE registry
/// and create the index BEFORE persisting (review 066 atomicity).
fn indexed_field_to_create(
    registry: &SchemaRegistry,
    collection: &str,
) -> Option<(String, CreateIndexKind)> {
    let col = registry.collection(collection)?;
    let field = col.fields().last()?;
    if !field.indexed() {
        return None;
    }
    Some((field.field_id().to_string(), index_kind_for(field)))
}

/// The serde `op` tag for a [`SchemaChange`] (for the response/event payload).
fn change_op(change: &SchemaChange) -> &'static str {
    match change {
        SchemaChange::AddCollection { .. } => "add_collection",
        SchemaChange::AddField { .. } => "add_field",
        SchemaChange::RenameField { .. } => "rename_field",
        SchemaChange::WidenField { .. } => "widen_field",
        SchemaChange::DeprecateField { .. } => "deprecate_field",
        SchemaChange::EnforceRequired { .. } => "enforce_required",
    }
}

/// A compact JSON summary of the registry for the `schema.apply_change` response:
/// each collection with its fields' stable ids, names, types, and flags. Lets a
/// shell confirm the minted ids / evolved state without re-reading the persisted
/// registry.
fn registry_summary(registry: &SchemaRegistry) -> serde_json::Value {
    let collections: serde_json::Map<String, serde_json::Value> = registry
        .collections()
        .map(|(name, col)| {
            let fields: Vec<serde_json::Value> = col
                .fields()
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "field_id": f.field_id(),
                        "name": f.name(),
                        "ty": f.ty(),
                        "indexed": f.indexed(),
                        "deprecated": f.deprecated(),
                        "required": f.required(),
                        "enforced": f.enforced(),
                    })
                })
                .collect();
            (name.to_string(), serde_json::json!({ "fields": fields }))
        })
        .collect();
    serde_json::json!({ "collections": collections })
}

/// Parse the optional `index_ids` filter for `schema.rebuild_indexes`: an array
/// of index-id strings, or absent for "all". A present-but-malformed value is a
/// `ValidationError`.
fn parse_index_ids(cmd: &forge_domain::CoreCommand) -> Result<Option<Vec<String>>> {
    match cmd.payload.get("index_ids") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for entry in arr {
                let s = entry.as_str().ok_or_else(|| {
                    CoreError::ValidationError(
                        "schema.rebuild_indexes `index_ids` entries must be strings".into(),
                    )
                })?;
                out.push(s.to_string());
            }
            Ok(Some(out))
        }
        Some(other) => Err(CoreError::ValidationError(format!(
            "schema.rebuild_indexes `index_ids` must be an array of strings, got {other}"
        ))),
    }
}
