# Review: d0503f09 delete timestamp restore-clock follow-up

## Findings

- **P1 - Synced deletes still lose the timestamp that the restore clock now depends on.** The local path now carries delete `logical_at` as oplog `mutation_at`, and `monotone_restore_clock` relies on `record_history(...).logical_at` to include that delete in the frontier (`forge/crates/core/src/commands/time_travel.rs:188-206`). But remote imports explicitly set `mutation_at: None` (`forge/crates/storage/src/crdt_write/oplog.rs:174-182`), and `import_remote_chunk_tx` has no way to pass it when writing the receiver's `record.remote_import` row (`forge/crates/storage/src/crdt_write/remote.rs:251-259`). The sync staging metadata also drops it while recovering row metadata (`forge/crates/sync/src/lib.rs:350-360`). So a peer that imports `insert@1 -> patch@2 -> delete@100` will still show the tombstone row with `logical_at=null`, and an omitted `db.restore` on that peer will default to `3`, before the delete it reverses. Please thread the origin row's `mutation_at` through `OplogEntry`/`RemoteChunk`/`remote_import`, and add a sync regression where the receiving peer restores after an imported late delete.

## Checks

- `cargo test -p forge-core --test time_travel_command --offline`
- `cargo test -p forge-storage --test time_travel_fixtures --offline`
- `cargo test -p forge-storage delete_version_reports --offline`
- `cargo test -p forge-core --test sync_rbac_enforced forwarded_chunk --offline`
