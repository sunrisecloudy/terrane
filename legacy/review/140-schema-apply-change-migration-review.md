# Commit Review: ee084735

Reviewed commit: `ee084735 forge-core: drive durable record migration from schema.apply_change (DL-13 M2)`

## Findings

### P1 - Indexed `add_field` defaults populate a different field id than the created index

In `forge/crates/core/src/commands/schema.rs:85`, an indexed `AddField` creates an index from `indexed_field_to_create`, which returns the registry-minted stable id (`field.field_id()`, e.g. `f_fx_1`) at `forge/crates/core/src/commands/schema.rs:313`. The new default-fill migration then writes existing records under the M0a stand-in id `record_field_id(&name)` (`f_<name>`, e.g. `f_currency`) at `forge/crates/core/src/commands/schema.rs:385` and `forge/crates/core/src/commands/schema.rs:456`.

That means an indexed field added with a default has migrated data under `field_ids["f_currency"]`, while the advertised/created index is `idx_records_items_f_fx_1`. Queries and rebuilds using the schema field id miss the defaulted rows or maintain an empty/wrong index. The current tests cover indexed fields without defaults and defaulted fields without indexes, but not the combined path.

Suggested fix: until storage writes registry-aware ids, make the created index use the same id that migrations and existing records actually carry, or migrate/materialize records to the registry stable id before creating/rebuilding the index. Add a regression for `indexed: true` plus `default`: seed an existing record, apply the change, then assert the row is found by the advertised field id and the planner uses the expected index.

### P2 - `RenameField` still skips the record migration path

`FieldTransform::RenameField` exists and is documented as the display-projection migration in `forge/crates/schema/src/migration.rs:40`, but `migration_for_change` returns `None` for `SchemaChange::RenameField` at `forge/crates/core/src/commands/schema.rs:415`. So `schema.apply_change(rename_field)` only bumps `schema_version` and persists the registry; existing records keep the old display key and the old `f_<name>` stand-in that the M0a mutation path materializes.

Suggested fix: build a `RenameField` descriptor for command-level renames, using the old name from the pre-change registry and the new name from the change/candidate registry. Add a test that renames a field with an existing record and verifies both the display projection and durable rebuild result.

## Verification

Not run; static heartbeat review only, with unrelated dirty worktree changes preserved.
