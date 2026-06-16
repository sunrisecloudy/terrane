# Commit Review: 2cb0efc8

Reviewed commit: `2cb0efc8 forge-storage/sync: migration chunks sync to peers + advance receiver schema_version (DL-13 review 139)`

## Findings

### P1 - Migration chunks advance schema state with only record-write authorization

The sync seam now recovers a migration chunk's `to` version from a `schema.migration` oplog row (`forge/crates/sync/src/lib.rs:284`) and passes it to `RemoteChunk.schema_version` (`forge/crates/sync/src/lib.rs:430`); import then advances the receiver's persisted `schema_version` in the same transaction (`forge/crates/storage/src/crdt_write/remote.rs:187`). But the core authorizer still translates every staged chunk as a record op: `SyncRecordOp::Write` becomes `RemoteOp::Insert`, `resource_type` is always `Record`, and `schema_version` is dropped from the authorization envelope (`forge/crates/core/src/workspace.rs:744`, `forge/crates/core/src/workspace.rs:752`, `forge/crates/core/src/workspace.rs:774`). The new core regression even codifies an `Editor` with only `db.write` on `expenses` as sufficient to import a migration and bump schema version (`forge/crates/core/tests/sync_rbac_enforced.rs:495`, `forge/crates/core/tests/sync_rbac_enforced.rs:508`).

That contradicts the sync RBAC contract: incoming metadata includes schema version, and schema-changing operations require Owner/Maintainer plus `schema_write` (`forge/spec/sync-rbac.md:90`, `forge/spec/sync-rbac.md:106`, `forge/spec/sync-rbac.md:124`). A db-only writer can now push a chunk that mutates receiver schema metadata, even though the same actor would not be allowed to perform a schema change at the command boundary.

Suggested fix: treat `schema_version: Some(_)` / `schema.migration` chunks as schema-affecting ops at the apply gate. Require the normal record-write grant for the migrated collection plus schema-change authorization (`Owner`/`Maintainer` and `schema_write`), or split the envelope into an explicit record-write + schema-change decision before import. Add tests that an Editor with only `db.write` is denied for migration chunks, while a trusted Maintainer/Owner with `schema_write` and collection write is allowed.

### P1 - Receiver version can advance while its schema registry stays old

PRD 02 says the schema registry is a CRDT document and schema changes sync like data (`prd-merged/02-data-layer-prd.md:15`). This commit syncs the migration record chunk and advances `Store::schema_version`, but `WorkspaceCore::sync_with` only hands the stores/index managers and membership tables to `forge_sync` (`forge/crates/core/src/workspace.rs:506`, `forge/crates/core/src/workspace.rs:521`); the receiver's `SchemaRegistry` is not updated or rebuilt. The new core test asserts migrated record values and `store().schema_version() == 2`, but never asserts `receiver.registry()` contains the corresponding evolved schema (`forge/crates/core/tests/sync_rbac_enforced.rs:545`, `forge/crates/core/tests/sync_rbac_enforced.rs:551`).

So a real `schema.apply_change` migration can leave a peer with data and `schema_version` at v2 while its registry remains v1/empty. Later validation, index reconstruction, and subsequent schema changes run against the stale registry even though the version anchor says the peer is current.

Suggested fix: sync the schema registry change as part of the same logical schema migration, or do not advance a receiver's schema version until the matching registry update has been imported and accepted. Add a core-level test that applies a schema change through `schema.apply_change` on the sender, syncs to a receiver, and asserts migrated records, `schema_version`, `receiver.registry()`, and any indexed-field reconstruction all agree.

## Verification

Not run; static heartbeat review only, with unrelated dirty worktree changes preserved.
