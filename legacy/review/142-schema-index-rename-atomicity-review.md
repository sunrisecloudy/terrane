# Commit Review: 5fc17606

Reviewed commit: `5fc17606 forge-core/storage: atomic registry+migration transaction + M0a f_<name> index convergence (DL-13 reviews 140-atomicity/141)`

## Findings

### P1 - Renaming an indexed field strands records under the old `f_<name>` key

The new M0a decision makes indexes/rebuilds follow the display-derived stand-in: `collection_indexed_fields` rebuilds an indexed field from `record_field_id(f.name())` (`forge/crates/core/src/commands/schema.rs:336`, `forge/crates/core/src/commands/schema.rs:343`), and new indexed fields are created on that same `f_<name>` key (`forge/crates/core/src/commands/schema.rs:373`, `forge/crates/core/src/commands/schema.rs:382`). But `RenameField` still treats the field id as display-only: the command builds a rename transform from `f_<old_name>` (`forge/crates/core/src/commands/schema.rs:464`, `forge/crates/core/src/commands/schema.rs:479`), and the transform ignores `field_id` and only moves `fields[old] -> fields[new]` (`forge/crates/schema/src/migration.rs:176`, `forge/crates/schema/src/migration.rs:188`).

So an indexed `label -> title` rename leaves existing records carrying `field_ids["f_label"]`, while the registry now rebuilds/reopens the index as `idx_records_tasks_f_title`. After a rebuild or reopen, queries/text-search by the field id that matches the current name (`f_title`) miss the migrated rows. In the same process, the old `f_label` index can also remain registered even though the registry now says the field is `title`.

Suggested fix: under the `f_<name>` M0a scheme, `RenameField` must also move the record-side stand-in from `f_<old_name>` to `f_<new_name>` and rebuild/update index definitions for indexed renames inside the schema transaction. Add a regression: create an indexed field, seed a record, rename it, rebuild/reopen, then assert `field_id: "f_<new_name>"` finds the row and the old key/index no longer serves the field.

### P2 - The new atomic schema transaction still excludes index creation

`cmd_schema_apply_change` still creates and registers the index before the new registry+migration transaction starts (`forge/crates/core/src/commands/schema.rs:85`, `forge/crates/core/src/commands/schema.rs:87`, then the transaction begins at `forge/crates/core/src/commands/schema.rs:136`). `IndexManager::create_index` mutates physical SQLite state and registers the definition immediately (`forge/crates/storage/src/index.rs:393`, `forge/crates/storage/src/index.rs:405`, `forge/crates/storage/src/index.rs:411`). If the later migration/registry-persist step fails, records, version, and registry roll back, but the physical index and in-memory manager entry do not.

That leaves a rejected `add_field indexed` with a live index for a field the registry never accepted, which contradicts the commit's all-or-nothing schema-change claim. The current registry-persist fault-injection test covers a widen path, so it does not exercise this pre-transaction index side effect.

Suggested fix: make index creation part of the same transaction as migration/version/registry persistence, or pre-validate the index id before the transaction and only create/register it after the commit can no longer fail. Add a fault-injection test for `indexed add_field + simulate_failure_stage: "registry_persist"` that asserts the registry, version, records, in-memory index manager, physical index/FTS table, and reopen state are all unchanged.

## Verification

Not run; static heartbeat review only, with unrelated dirty worktree changes preserved.
