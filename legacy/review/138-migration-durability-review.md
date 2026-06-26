# Commit Review: 64fa2a34

Reviewed commit: `64fa2a34 forge-storage/schema: deterministic atomic migration engine + migrations.md spec (DL-13)`

Line references below are for the committed `64fa2a34` snapshot, not the current dirty worktree.

## Findings

### P1 - Successful migrations are lost on DL-6 projection rebuild

`Store::apply_migration` rewrites only the derived `records` projection, then appends a `schema.migration` oplog row, bumps `schema_version`, and rebuilds indexes (`forge/crates/storage/src/migration.rs:122-141`). It never updates the CRDT chunks that remain the declared source of truth. But `rebuild_projection_tx` starts by deleting all projection rows and rematerializes them solely from `crdt_chunks` (`forge/crates/storage/src/crdt_write/rebuild.rs:40-65`), then rebuilds indexes; it does not replay `schema.migration` oplog entries. After a successful migration, any DL-6 rebuild will therefore restore the pre-migration record values while leaving `schema_version` and the migration oplog advanced. That violates the PRD requirement that projection rebuild from CRDT docs complete with zero diff (`prd-merged/02-data-layer-prd.md:49-51`) and makes migrated data non-durable under the recovery path.

Suggested fix: make the migration durable in the same source-of-truth stream that rebuild consumes, either by encoding the migration as CRDT updates/chunks or by teaching rebuild/import to replay `schema.migration` ops deterministically after CRDT materialization. Add a regression that applies a migration, calls `Store::rebuild_projection`, and asserts the migrated record values, schema version, oplog, and active indexes remain coherent.

### P2 - `drop_field` leaves stale display values for real stable field ids

The spec says `drop_field` removes the value from both `field_ids[field_id]` and the display projection (`forge/spec/migrations.md:47-51`), and the PRD says stable ids are actor-scoped ids like `f_<actor>_<seq>` plus a separate display name (`prd-merged/02-data-layer-prd.md:55-58`). The committed transform only carries `field_id` (`forge/crates/schema/src/migration.rs:43-46`) and removes the display entry by stripping `f_` and treating the suffix as the name (`forge/crates/schema/src/migration.rs:152-160`). For a normal id such as `f_alice_1` with display name `note`, this removes `fields["alice_1"]` and leaves `fields["note"]` behind, so the supposedly dropped value still appears in the record envelope.

Suggested fix: include the prior display name in `DropField`, or pass the registry mapping into the migration engine, and add a regression with `field_id = "f_alice_1"` plus `fields["note"]` proving both maps are cleaned.

## Verification

Not run: this heartbeat review was static, and the current worktree contains unrelated uncommitted changes plus an untracked storage migration repro file, so broad local results would not describe the commit snapshot cleanly.
