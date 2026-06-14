//! `schema.apply_change` / `schema.validate_compatibility` / `schema.rebuild_indexes`
//! — the dynamic-schema commands (DL-7/DL-8 → DL-5). Moved verbatim from
//! `workspace.rs` (/simplify #11a): the three handlers plus the registry/index
//! helpers they (and the open-time index reconstruction in `workspace.rs`) share.

use forge_domain::{CoreError, Result};
use forge_schema::{
    CollectionDef, FieldDef, FieldTransform, FieldType, MigrationDescriptor, SchemaChange,
    SchemaRegistry,
};
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
    ///
    /// DL-13: a schema change that EVOLVES an existing field's data (a `widen_field`,
    /// a `deprecate_field`, or an `add_field` carrying a `default`) is paired with a
    /// companion **record migration** so the stored records are carried forward to
    /// match the registry evolution (`forge/spec/migrations.md`: the registry change
    /// decides *what the schema is*; the migration decides *how existing data is
    /// carried forward*). The migration is driven through the durable
    /// [`Store::apply_migration`] engine — it rewrites the CRDT source of truth (so a
    /// DL-6 rebuild reproduces the migrated values) and bumps the persisted
    /// `schema_version`, all in ONE transaction.
    ///
    /// ATOMICITY (the lifecycle/lockstep lesson): the registry evolution + its record
    /// migration + the version advance are ONE unit. The migration runs BEFORE the
    /// registry is persisted, and because [`Store::apply_migration`] is all-or-nothing
    /// (a non-coercible value — a lossy narrow — rolls back the records, the
    /// `schema_version`, the migration chunk, and the oplog), a failed migration
    /// returns its typed error here with the registry NEVER persisted. So after a
    /// rejected change NOTHING is persisted: registry, records, and `schema_version`
    /// are all unchanged.
    ///
    /// VERSION LOCKSTEP: the storage `schema_version` is the single source of version
    /// truth and advances by exactly one per accepted change. A data-affecting change
    /// advances it via [`Store::apply_migration`] (one bump, inside the migration); a
    /// no-record-transform change (`add_collection`, `enforce_required`, a defaultless
    /// `add_field`) advances it directly. Never double-bumped.
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

        // DL-13: advance the workspace schema_version in lockstep and carry existing
        // records forward. The storage `schema_version` is the single version source
        // of truth; this change advances it from `current` to `current + 1`.
        //
        // A data-affecting change (widen / deprecate / add_field-with-default) builds
        // its companion MigrationDescriptor against the CANDIDATE registry (so display
        // names resolve to the post-change registry) and drives the DURABLE migration,
        // which rewrites the CRDT source of truth, materializes the projection, bumps
        // the version, and rebuilds active indexes — atomically. Run it BEFORE
        // persisting the registry: a failed (non-coercible) migration rolls itself
        // fully back and returns its typed error here, so the registry is never
        // persisted and nothing changed. A no-record-transform change advances the
        // version directly (a single bump, never two).
        let current = self.store.schema_version()?;
        let migrated = match migration_for_change(&change, &next, cmd, current)? {
            Some(descriptor) => {
                let outcome = self.store.apply_migration(&descriptor, &self.indexes)?;
                outcome.migrated_records
            }
            None => {
                self.store.advance_schema_version(current + 1)?;
                0
            }
        };

        // The index (if any) was created and the migration committed successfully —
        // now durably commit the evolved registry. Persist BEFORE swapping the
        // in-memory copy so the durable schema and the in-memory one never diverge.
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
            "schema_version": current + 1,
            "migrated_records": migrated,
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
    fn persist_registry(&mut self, registry: &SchemaRegistry) -> Result<()> {
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

/// Build the companion DL-13 [`MigrationDescriptor`] for a schema `change` that
/// evolves an existing field's stored data, or `None` for a change that touches
/// only the registry/validation surface (`add_collection`, `enforce_required`, a
/// defaultless `add_field`). The descriptor is built against the CANDIDATE registry
/// `next` (the post-change state) so display `name`s resolve to the evolved schema,
/// and migrates `from = current` `to = current + 1` so the version advances in
/// lockstep (`forge/spec/migrations.md`).
///
/// The supported-transforms mapping is normative (`forge/spec/migrations.md` §1):
/// - `widen_field{field_id, to}` → [`FieldTransform::WidenField`] (coerce the
///   stored value to the wider type, e.g. `5` → `5.0`).
/// - `deprecate_field{field_id}` → [`FieldTransform::DropField`]: deprecate's data
///   side drops the record VALUE while the schema field is retained via the
///   `deprecated` flag (DL-8 "deprecate + retain at the schema level"). The
///   `deprecate_field_ok` fixture pins only the registry-level retention
///   (`deprecated: true`), so dropping the value is consistent with both the spec
///   table and the fixture.
/// - `add_field` → [`FieldTransform::AddField`] (fill-if-missing) ONLY when the
///   command carries a `default`; without a default the new field is simply absent
///   on existing records until written, so no record is rewritten.
///
/// `add_collection` and `enforce_required` map to no transform.
///
/// **Record-key resolution (M0a stand-in).** A transform must target the
/// `field_id` the **records actually carry**. The M0a DL-4 mutation surface keys a
/// display field `<name>` under the projection stand-in `f_<name>`
/// (`forge_storage`'s `materialize_field_ids`: storage has no registry, so it has
/// no schema name→id map and writes the `f_<name>` stand-in), so a record written
/// before the change carries `f_<name>`, NOT the registry's actor-scoped
/// `f_<actor>_<seq>`. The schema change references the registry id; we resolve that
/// id to its display `name` in the candidate registry, then key the record
/// transform by the matching stand-in [`record_field_id`] so the migration rewrites
/// the value the record really holds (mirroring the `forge_storage` migration tests,
/// which target `f_<name>`). The display `name` is carried explicitly so both the
/// stable-id map and the display projection are rewritten exactly (review 138 P2).
fn migration_for_change(
    change: &SchemaChange,
    next: &SchemaRegistry,
    cmd: &forge_domain::CoreCommand,
    current: u64,
) -> Result<Option<MigrationDescriptor>> {
    let transform = match change {
        SchemaChange::WidenField { collection, field_id, to } => {
            let name = resolve_display_name(next, collection, field_id)?;
            Some((
                collection.clone(),
                FieldTransform::WidenField {
                    field_id: record_field_id(&name),
                    name,
                    to: to.clone(),
                },
            ))
        }
        SchemaChange::DeprecateField { collection, field_id } => {
            let name = resolve_display_name(next, collection, field_id)?;
            Some((
                collection.clone(),
                FieldTransform::DropField { field_id: record_field_id(&name), name },
            ))
        }
        SchemaChange::AddField { collection, .. } => {
            // A default (an optional companion to `change` in the command payload) is
            // filled into existing records that lack the freshly minted field. The
            // schema crate mints the id when applying the change, so resolve the new
            // field from the CANDIDATE registry: it is the last field of `collection`.
            // The record-side fill targets the `f_<name>` stand-in (the key records
            // carry) so a later read sees the default under the same id.
            match cmd.payload.get("default") {
                None | Some(serde_json::Value::Null) => None,
                Some(default) => {
                    let name = next
                        .collection(collection)
                        .and_then(|col| col.fields().last())
                        .map(|f| f.name().to_string())
                        .ok_or_else(|| {
                            CoreError::ValidationError(format!(
                                "schema.apply_change: add_field default for unknown collection {collection:?}"
                            ))
                        })?;
                    Some((
                        collection.clone(),
                        FieldTransform::AddField {
                            field_id: record_field_id(&name),
                            name,
                            default: default.clone(),
                        },
                    ))
                }
            }
        }
        // Registry-only / validation-only changes carry no record transform.
        SchemaChange::AddCollection { .. }
        | SchemaChange::RenameField { .. }
        | SchemaChange::EnforceRequired { .. } => None,
    };

    Ok(transform.map(|(collection, transform)| MigrationDescriptor {
        collection,
        from_schema_version: current,
        to_schema_version: current + 1,
        transforms: vec![transform],
    }))
}

/// Resolve a field's current display name from the (candidate) registry by its
/// stable `field_id` (DL-7). The migration transform carries the display `name`
/// EXPLICITLY so the record's display projection is updated exactly (never an
/// `f_`-strip guess — review 138 P2). An unknown field is a `ValidationError`
/// (the change just minted/targeted it, so it must resolve).
fn resolve_display_name(
    registry: &SchemaRegistry,
    collection: &str,
    field_id: &str,
) -> Result<String> {
    registry
        .collection(collection)
        .and_then(|col| col.field(field_id))
        .map(|f| f.name().to_string())
        .ok_or_else(|| {
            CoreError::ValidationError(format!(
                "schema.apply_change: field {field_id:?} not found in collection {collection:?}"
            ))
        })
}

/// The stable `field_id` a record carries for display field `name` in the M0a
/// mutation surface: the `f_<name>` stand-in. This mirrors `forge_storage`'s
/// `materialize_field_ids` (kept in lockstep): storage has no schema name→id map,
/// so a record written through the DL-4 path keys its value under `f_<name>`. A
/// record migration must target the SAME key to rewrite the value the record
/// actually holds, so the companion descriptor keys by this stand-in.
fn record_field_id(name: &str) -> String {
    format!("f_{name}")
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
