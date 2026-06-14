//! `schema.apply_change` / `schema.validate_compatibility` / `schema.rebuild_indexes`
//! — the dynamic-schema commands (DL-7/DL-8 → DL-5). Moved verbatim from
//! `workspace.rs` (/simplify #11a): the three handlers plus the registry/index
//! helpers they (and the open-time index reconstruction in `workspace.rs`) share.

use forge_domain::{CoreError, Result};
use forge_schema::{
    CollectionDef, FieldDef, FieldTransform, FieldType, MigrationDescriptor, SchemaChange,
    SchemaRegistry,
};
use forge_storage::{kv_set_tx, CreateIndexKind, IndexDef, IndexState};

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

        // DL-8 → DL-5: the storage-index work a change implies (review 142 P2). An
        // `add_field{indexed}` CREATEs the field's index; a `rename_field` of an
        // INDEXED field SWAPs the index from the old `f_<old_name>` stand-in to the new
        // `f_<new_name>` (the index is name-derived, so a rename moves it — review 142
        // P1). Computed against the CANDIDATE registry BEFORE the transaction so an
        // un-indexable field id (`IndexDef::new` validates it) rejects the whole
        // `apply_change` here with the live + persisted registry untouched (review 066
        // non-poisoning) — the create itself happens INSIDE the transaction below.
        let index_op = index_op_for_change(&change, &self.registry, &next)?;

        // DL-13: advance the workspace schema_version in lockstep, carry existing
        // records forward, create/swap the field's storage index, AND durably persist
        // the evolved registry — as ONE atomic unit. The storage `schema_version` is
        // the single version source of truth; this change advances it from `current`
        // to `current + 1`.
        //
        // CROSS-TRANSACTION ATOMICITY (reviews 140/142): the record migration / version
        // advance, the index create/swap, and the registry persist must commit OR roll
        // back TOGETHER. Previously the index was created in its OWN connection BEFORE
        // this transaction, so a later migration/registry failure rolled records +
        // version + registry back but left the physical index + in-memory manager entry
        // live — a rejected indexed `add_field` leaving a live index (review 142 P2). So
        // ALL of it now runs in ONE `Store::transact`:
        //   - the index create/swap runs first via `create_index_tx` / `drop_index` on
        //     the tx (so the physical DDL rolls back with the tx);
        //   - a data-affecting change (widen / deprecate / add_field-with-default /
        //     indexed-rename) drives the DURABLE migration in-tx (rewrites the CRDT
        //     source of truth, materializes the projection, bumps the version, and
        //     rebuilds active indexes — so the just-created index is built from the
        //     migrated rows);
        //   - a no-record-transform change advances the version directly (one bump) and
        //     builds the just-created index from the current projection;
        //   - then the evolved registry is persisted via `kv_set_tx` in the SAME tx.
        // A failure ANYWHERE rolls EVERYTHING back. The PHYSICAL index is reverted by
        // SQLite's rollback; the IN-MEMORY `IndexManager` is not transactional, so we
        // snapshot it before the tx and `restore` on failure (review 142 P2).
        let current = self.store.schema_version()?;
        let descriptor = migration_for_change(&change, &self.registry, &next, cmd, current)?;
        // DL-13 review 143: when this change drives a record migration, carry the
        // affected collection's EVOLVED registry entry onto the migration chunk so an
        // authorized receiver evolves its SchemaRegistry in lockstep (the registry is a
        // CRDT document that syncs with the migration — prd-merged/02:15). The entry is
        // `{ name, collection }` taken from the CANDIDATE registry `next` (the
        // post-change state), serialized once here; `None` when no migration runs (a
        // no-record-transform change carries no chunk to ride).
        let migration_registry_collection = descriptor
            .as_ref()
            .map(|d| registry_collection_entry(&next, &d.collection))
            .transpose()?;
        let registry_bytes = serde_json::to_vec(&next)
            .map_err(|e| CoreError::StorageError(format!("serialize schema registry: {e}")))?;
        let peer_id = self.store.crdt_peer_id();
        // TEST-ONLY seam (mirrors `applet.upgrade`'s `simulate_failure_stage`):
        // inject a failure at the registry-persist step INSIDE the transaction —
        // AFTER the migration / version advance / index create committed-in-tx — so the
        // review-140/142 fault-injection tests prove the records + `schema_version` +
        // index roll back with the registry persist, not merely that "all writes
        // happened to succeed". Compiles to a one-shot bool from the payload; absent in
        // normal use.
        let simulate_registry_persist_failure = cmd
            .payload
            .get("simulate_failure_stage")
            .and_then(|v| v.as_str())
            == Some("registry_persist");

        // Snapshot the in-memory IndexManager so an in-tx index create/swap can be
        // rolled back in memory if the transaction fails (the physical side rolls back
        // with the tx; the manager's `defs` map is not transactional — review 142 P2).
        let index_snapshot = self.indexes.snapshot();
        // Disjoint field borrows (mirrors `WorkspaceCore::rebuild_projection`): the
        // closure mutates `self.indexes` (index create/swap) and reborrows it as shared
        // for the migration; `transact` borrows `self.store`.
        let indexes = &mut self.indexes;
        let outcome = self.store.transact(|tx| {
            // (a) Create/swap the field's storage index INSIDE the tx, so the physical
            // DDL rolls back with everything else (review 142 P2). For an indexed
            // rename, drop the old `f_<old_name>` index first so it no longer serves the
            // field, then create the new one.
            let mut created_index = None;
            if let Some(op) = &index_op {
                for drop in &op.drops {
                    indexes.drop_index(tx, &op.collection, &drop.field_id, drop.kind)?;
                }
                if let Some(create) = &op.create {
                    let id = indexes.create_index_tx(
                        tx,
                        &op.collection,
                        &create.field_id,
                        create.kind,
                    )?;
                    created_index = Some(id);
                }
            }

            // (b) Carry existing records forward + bump the version (data-affecting
            // change), or advance the version directly (no-record-transform change).
            // The migration's `rebuild_active` rebuilds the just-created index from the
            // migrated projection; the no-migration branch builds it from the current
            // projection so the new index is populated either way.
            let migrated = match &descriptor {
                Some(descriptor) => {
                    let outcome = forge_storage::apply_migration_in_tx(
                        tx,
                        descriptor,
                        peer_id,
                        indexes,
                        // Carry the evolved collection entry onto the migration chunk so
                        // an authorized receiver evolves its registry in lockstep (review
                        // 143). The same entry is persisted locally via the registry
                        // kv_set below; both commit/roll-back together in this one txn.
                        migration_registry_collection.as_ref(),
                    )?;
                    outcome.migrated_records
                }
                None => {
                    forge_storage::advance_schema_version_tx(tx, current + 1)?;
                    // No migration ran `rebuild_active`, so build the just-created index
                    // from the current projection (a no-default indexed add_field).
                    if index_op.as_ref().and_then(|o| o.create.as_ref()).is_some() {
                        indexes.rebuild_active(tx)?;
                    }
                    0
                }
            };

            // Injected registry-persist failure: AFTER the migration / version advance /
            // index create ran in this tx but in place of the registry kv_set. Returning
            // `Err` rolls the WHOLE transaction back — the migrated records, the
            // `schema_version`, AND the physical index included — so the schema can never
            // end up behind the data, and a rejected change leaves no live index
            // (reviews 140/142).
            if simulate_registry_persist_failure {
                return Err(CoreError::StorageError(
                    "simulated registry-persist failure after the record migration".into(),
                ));
            }
            // Persist the evolved registry in the SAME tx as the migration / version
            // advance / index create: the fault-injection above proves a failure here
            // rolls all of it back together.
            persist_registry_tx(tx, &registry_bytes)?;
            Ok((migrated, created_index))
        });

        let (migrated, created_index) = match outcome {
            Ok(v) => v,
            Err(e) => {
                // The tx rolled back the physical index; restore the in-memory manager
                // so a rejected change leaves no live in-memory index entry (review 142
                // P2). The borrow of `self.indexes` ended with the closure.
                self.indexes.restore(index_snapshot);
                return Err(e);
            }
        };

        // Everything committed — swap the in-memory copy so the durable schema and the
        // in-memory one never diverge.
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

}

/// Persist the serialized registry to the workspace file (`__forge/meta` /
/// `schema_registry`) INSIDE a caller-provided transaction, so it commits or rolls
/// back together with the record migration + version advance (cross-transaction
/// atomicity, review 140). A free function (not a `&mut self` method) so it can run
/// inside a `self.store.transact` closure while `self.indexes` is disjointly
/// borrowed by the migration. The registry is pre-serialized by the caller (a
/// serialization failure should reject BEFORE the tx opens).
fn persist_registry_tx(tx: &forge_storage::Transaction<'_>, bytes: &[u8]) -> Result<()> {
    kv_set_tx(tx, META_NS, SCHEMA_REGISTRY_KEY, bytes, "application/json")
}

/// Build the migration's carried registry entry for `collection` from the CANDIDATE
/// (post-change) registry: `{ "name": <collection>, "collection": <CollectionDef> }`
/// (DL-13 review 143). The migration chunk carries this so an authorized receiver
/// evolves its `SchemaRegistry` in lockstep with the migrated records + version,
/// instead of leaving the receiver at version N with N-shaped data under an N-1
/// registry (the registry-drift this closes). Naming the collection explicitly lets
/// the receiver replace exactly that entry without re-deriving the name from the
/// chunk's doc id. The collection MUST exist in `next` (the change just evolved it),
/// so an absent entry is a `ValidationError` rather than a silently dropped carry.
fn registry_collection_entry(next: &SchemaRegistry, collection: &str) -> Result<serde_json::Value> {
    let col = next.collection(collection).ok_or_else(|| {
        CoreError::ValidationError(format!(
            "schema.apply_change: migrated collection {collection:?} not found in the evolved registry"
        ))
    })?;
    let collection_value = serde_json::to_value(col).map_err(|e| {
        CoreError::StorageError(format!("serialize migrated collection {collection:?}: {e}"))
    })?;
    Ok(serde_json::json!({ "name": collection, "collection": collection_value }))
}

/// Every `(collection, field_id, kind)` the registry declares as `indexed`,
/// skipping deprecated fields (a hidden field's index is not maintained). The
/// kind is derived from the field type ([`index_kind_for`]). Stable iteration
/// order (registry collections are a `BTreeMap`, fields are declaration-ordered).
///
/// M0a FIELD-ID CONVERGENCE (review 141): the `field_id` is the `f_<name>`
/// **stand-in** ([`record_field_id`]) — the SAME id the apply-time index, the
/// default-fill, and the write path's `materialize_field_ids` all use. So the
/// open-time index reconstruction and `schema.rebuild_indexes` rebuild EXACTLY the
/// index `schema.apply_change` created, over the id records actually carry (no
/// divergence between create-time and reopen-time index ids).
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
/// `(collection, field_id, kind)`, keyed by the `f_<name>` stand-in (review 141).
fn collection_indexed_fields(
    name: &str,
    col: &CollectionDef,
) -> Vec<(String, String, CreateIndexKind)> {
    col.fields()
        .iter()
        .filter(|f| f.indexed() && !f.deprecated())
        .map(|f| (name.to_string(), record_field_id(f.name()), index_kind_for(f)))
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

/// The (record-side field id, index kind) for the last-added field of `collection`
/// in `registry`, iff that field is marked `indexed` (DL-8 → DL-5). Takes the
/// registry explicitly so `schema.apply_change` can probe the CANDIDATE registry
/// and create the index BEFORE persisting (review 066 atomicity).
///
/// M0a FIELD-ID CONVERGENCE (review 141): the index is built over the `f_<name>`
/// **stand-in** ([`record_field_id`]) — the SAME id the applet/CRDT write path
/// materializes display values under ([`materialize_field_ids`]) — NOT the
/// registry's actor-scoped stable id (`f_<actor>_<seq>`). The storage write path
/// is not registry-aware, so a later DL-4 display write of the same field lands on
/// `f_<name>`; if the index keyed the registry id instead, the index would serve a
/// stale value (or nothing) while display reads saw the new one — a SPLIT IDENTITY
/// (review 141 P1). Keying the index, the default-fill, and every migration on the
/// one `f_<name>` stand-in keeps a single id scheme in M0a (see DECISIONS.md);
/// registry-aware materialization is deferred (DL-7 future work).
fn indexed_field_to_create(
    registry: &SchemaRegistry,
    collection: &str,
) -> Option<(String, CreateIndexKind)> {
    let col = registry.collection(collection)?;
    let field = col.fields().last()?;
    if !field.indexed() {
        return None;
    }
    Some((record_field_id(field.name()), index_kind_for(field)))
}

/// One storage index to create over a `(collection, field_id)` of a given kind.
struct IndexCreate {
    field_id: String,
    kind: CreateIndexKind,
}

/// One storage index to drop (its physical structure + in-memory def of `kind`).
struct IndexDrop {
    field_id: String,
    kind: CreateIndexKind,
}

/// The storage-index work a schema `change` implies (DL-8 → DL-5; review 142 P2),
/// all keyed by the `f_<name>` stand-in ([`record_field_id`]) so the index, the
/// write path, and queries share one id (review 141). `None` for a change that
/// touches no index. The `create`/`drops` are PERFORMED inside the schema
/// transaction; this just describes (and pre-validates) the work.
struct IndexOp {
    collection: String,
    /// Old index(es) to drop — non-empty only for an INDEXED rename, which moves the
    /// index from the old `f_<old_name>` stand-in to the new `f_<new_name>`.
    drops: Vec<IndexDrop>,
    /// The index to create (the new field's index, or the renamed field's moved one).
    create: Option<IndexCreate>,
}

/// Decide the storage-index work a `change` implies, pre-validating any new index id
/// so an un-indexable field NAME rejects the whole `apply_change` BEFORE the
/// transaction (review 066 non-poisoning) — the actual create/drop runs in-tx.
///
/// - `add_field{indexed}` → CREATE the field's index over its `f_<name>` stand-in.
/// - `rename_field` of an INDEXED field → SWAP: DROP the old `f_<old_name>` index
///   and CREATE the new `f_<new_name>` one. Under the M0a name-derived stand-in a
///   rename moves the record-side id, so the index must move with it or it would
///   serve the OLD key while the rows moved to the new one (review 142 P1). The OLD
///   display name is read from `prev` (`next` already renamed it).
/// - every other change touches no index.
fn index_op_for_change(
    change: &SchemaChange,
    prev: &SchemaRegistry,
    next: &SchemaRegistry,
) -> Result<Option<IndexOp>> {
    let op = match change {
        SchemaChange::AddField { collection, indexed: true, .. } => {
            let Some((field_id, kind)) = indexed_field_to_create(next, collection) else {
                return Ok(None);
            };
            // Pre-validate the stand-in index id (an un-indexable field NAME like
            // `ti@tle` → `f_ti@tle` rejects here, before the tx — review 066).
            validate_index_id(collection, &field_id, kind)?;
            IndexOp {
                collection: collection.clone(),
                drops: Vec::new(),
                create: Some(IndexCreate { field_id, kind }),
            }
        }
        SchemaChange::RenameField { collection, field_id, name } => {
            // Only an INDEXED field's rename touches an index. Resolve the field in
            // `next` by its stable id (the rename kept the id, changed the name).
            let Some(field) = next.collection(collection).and_then(|c| c.field(field_id)) else {
                return Ok(None);
            };
            if !field.indexed() {
                return Ok(None);
            }
            let kind = index_kind_for(field);
            let old_name = resolve_display_name(prev, collection, field_id)?;
            // A same-name rename moves nothing (the registry no-ops it too).
            if &old_name == name {
                return Ok(None);
            }
            let old_field_id = record_field_id(&old_name);
            let new_field_id = record_field_id(name);
            validate_index_id(collection, &new_field_id, kind)?;
            IndexOp {
                collection: collection.clone(),
                drops: vec![IndexDrop { field_id: old_field_id, kind }],
                create: Some(IndexCreate { field_id: new_field_id, kind }),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(op))
}

/// Validate that a `(collection, field_id, kind)` mints a well-formed storage index
/// id, so an un-indexable identifier rejects the schema change BEFORE the
/// transaction opens (review 066 non-poisoning). Constructing an [`IndexDef`] runs
/// the same identifier allowlist `create_index` would, without emitting any DDL.
fn validate_index_id(collection: &str, field_id: &str, kind: CreateIndexKind) -> Result<()> {
    IndexDef::new(collection, field_id, kind.into(), IndexState::Active).map(|_| ())
}

/// Build the companion DL-13 [`MigrationDescriptor`] for a schema `change` that
/// evolves an existing field's stored data, or `None` for a change that touches
/// only the registry/validation surface (`add_collection`, `enforce_required`, a
/// defaultless `add_field`). The descriptor is built against the CANDIDATE registry
/// `next` (the post-change state) so display `name`s resolve to the evolved schema;
/// the PRE-change registry `prev` is read for a `rename_field`'s OLD display name
/// (which `next` has already overwritten). The migration runs `from = current`
/// `to = current + 1` so the version advances in lockstep (`forge/spec/migrations.md`).
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
/// - `rename_field{field_id, name}` → [`FieldTransform::RenameField`]: MOVE both the
///   record-side stand-in `f_<old_name>` → `f_<new_name>` AND the display projection
///   key OLD name → NEW `name` (the OLD name read from `prev`). Under the M0a
///   name-derived stand-in the rename is NOT display-only — the record's value id
///   moves with the name, so a stale `f_<old_name>` would otherwise strand the value
///   while the index / write path / query all moved to `f_<new_name>` (review 142 P1;
///   review 140 P2 first drove the rename's display move).
/// - `add_field` → [`FieldTransform::AddField`] (fill-if-missing) ONLY when the
///   command carries a `default`; without a default the new field is simply absent
///   on existing records until written, so no record is rewritten.
///
/// `add_collection` and `enforce_required` map to no transform.
///
/// **Record-key resolution (the M0a `f_<name>` stand-in — reviews 140/141).** A
/// transform must target the `field_id` the **records actually carry**. The M0a DL-4
/// mutation surface keys a display field `<name>` under the projection stand-in
/// `f_<name>` (`forge_storage`'s `materialize_field_ids`: storage has no registry,
/// so it has no schema name→id map and writes the `f_<name>` stand-in), so a record
/// written through the DL-4 path carries `f_<name>`, NOT the registry's actor-scoped
/// `f_<actor>_<seq>`. So EVERY transform — `widen`/`deprecate` over an existing
/// display field, and an `add_field` default whether or not the field is indexed —
/// keys by the stand-in [`record_field_id`].
///
/// CONVERGED ID SCHEME (review 141): an `indexed` `add_field` builds its storage
/// index over the SAME `f_<name>` stand-in ([`indexed_field_to_create`]), so filling
/// the default under that stand-in populates the index AND means a later DL-4 display
/// write of the same name updates the SAME key — no split identity. (Round 2 keyed an
/// indexed default-fill under the registry stable id `f_<actor>_<seq>` to match an
/// index built over that id; but the write path is not registry-aware, so a later
/// `f_<name>` display write left the index serving a stale value — review 141 P1.
/// That direction is reverted: indexes, default-fills, and migrations all key by the
/// stand-in in M0a; registry-aware materialization is deferred DL-7 work — see
/// `prd-merged/DECISIONS.md`.)
///
/// The display `name` (and `from_name`/`to_name` for a rename) is carried explicitly
/// so both the stable-id map and the display projection are rewritten exactly (review
/// 138 P2) — never an `f_`-strip guess.
fn migration_for_change(
    change: &SchemaChange,
    prev: &SchemaRegistry,
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
        SchemaChange::RenameField { collection, field_id, name } => {
            // Review 142 P1: under the M0a `f_<name>` stand-in scheme the record-side
            // id is NAME-DERIVED, so a rename is NOT display-only — it MOVES the
            // stand-in `f_<old_name>` → `f_<new_name>` on existing records (so the
            // value follows the field instead of being stranded under the old key while
            // the index / write path / query all move to `f_<new_name>`). The OLD
            // display name lives in `prev` (`next` already renamed it); the NEW name is
            // the change's `name`. A same-name rename (idempotent registry no-op)
            // carries no record change.
            let from_name = resolve_display_name(prev, collection, field_id)?;
            if &from_name == name {
                None
            } else {
                Some((
                    collection.clone(),
                    FieldTransform::RenameField {
                        // Both record-side stand-ins so the value moves to the field's
                        // new name-derived id (NOT the actor-scoped registry id, which
                        // the write path is unaware of — review 141 single-id scheme).
                        from_field_id: record_field_id(&from_name),
                        to_field_id: record_field_id(name),
                        from_name,
                        to_name: name.clone(),
                    },
                ))
            }
        }
        SchemaChange::AddField { collection, .. } => {
            // A default (an optional companion to `change` in the command payload) is
            // filled into existing records that lack the freshly minted field. The
            // schema crate mints the id when applying the change, so resolve the new
            // field from the CANDIDATE registry: it is the last field of `collection`.
            match cmd.payload.get("default") {
                None | Some(serde_json::Value::Null) => None,
                Some(default) => {
                    let field = next
                        .collection(collection)
                        .and_then(|col| col.fields().last())
                        .ok_or_else(|| {
                            CoreError::ValidationError(format!(
                                "schema.apply_change: add_field default for unknown collection {collection:?}"
                            ))
                        })?;
                    let name = field.name().to_string();
                    // M0a FIELD-ID CONVERGENCE (review 141): fill the default under the
                    // `f_<name>` stand-in for EVERY field — indexed or not. The index
                    // (when the field is `indexed`) is built over the SAME stand-in
                    // (`indexed_field_to_create`), and a later DL-4 display write of the
                    // same name also lands on `f_<name>`. Keying the default-fill,
                    // the index, and every migration on the one stand-in keeps a single
                    // id scheme, so the index serves the defaulted rows AND a later
                    // display write of the same field updates the SAME key (no split
                    // identity — the round-2 registry-id direction is reverted).
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
        SchemaChange::AddCollection { .. } | SchemaChange::EnforceRequired { .. } => None,
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
