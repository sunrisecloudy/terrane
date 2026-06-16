# Commit Review: ed9be371

Reviewed commit: `ed9be371 forge-storage/schema: make DL-13 migration durable in CRDT source-of-truth + exact display-name transforms (review 138)`

Line references below are for the committed `ed9be371` snapshot, not the current dirty worktree.

## Findings

### P1 - Migration chunks are not syncable because they have no per-chunk oplog metadata, and remote import cannot advance schema_version

The follow-up correctly persists the migration as a CRDT chunk (`forge/crates/storage/src/crdt_write/mutation.rs:423-426`), but it records only a `schema.migration` oplog row keyed as `migration#<from>-<to>#<collection>` (`forge/crates/storage/src/migration.rs:152-190`). The sync staging path joins chunks back to metadata by `op_id = "{doc_id}#{local_chunk_id}"` (`forge/crates/sync/src/lib.rs:311-329`). Because the migration chunk has no matching `collection/<name>#chunk-NNNN` oplog row, sync falls back to a generic record write with `record_ids = []`; the existing RBAC gate denies record writes with empty record ids (`forge/crates/core/src/sync_rbac.rs:375-380`). So the new migration chunk survives local DL-6 rebuild, but it is dropped at peer sync and never reaches another device.

There is a second half to the same boundary: even if that chunk were allowed, remote import only writes the chunk plus a `record.remote_import` oplog row (`forge/crates/storage/src/crdt_write/remote.rs:131-172`). It does not apply the `schema.migration` metadata or call the local schema-version bump (`forge/crates/storage/src/migration.rs:135-139`), so a receiver could materialize migrated records while remaining at the old `schema_version`.

Suggested fix: make migrations first-class in the synced metadata path. Either append a per-chunk oplog row keyed `collection/<name>#chunk-NNNN` with record ids and migration version metadata, or teach `missing_chunks_for_doc`/`envelope_for_chunk` how to associate a migration chunk with its `schema.migration` row. Then, on authorized remote apply, advance the receiving store's schema version (and preserve the migration audit metadata) in the same transaction as the chunk import/rebuild. Add a two-store regression: A migrates, `sync_stores(A,B)` moves the migration chunk, B has the migrated values after rebuild, B's `schema_version == to_schema_version`, and no chunk is denied.

## Verification

Not run: this heartbeat review was static, and the worktree contains unrelated uncommitted changes.
