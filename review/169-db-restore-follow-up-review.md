# Review: c7032c32 db.restore notification + clock follow-up

## Findings

- **P2 - Omitted restore clocks still ignore delete timestamps.** `cmd_db_restore` now defaults `restored_logical_at` via `monotone_restore_clock`, but that helper only takes the max non-null `logical_at` from `record_history(...)` (`forge/crates/core/src/commands/time_travel.rs:177-198`). Deletes are exactly the case where history reports `logical_at = None` because no envelope survives (`forge/crates/storage/src/time_travel.rs:133-135`, `forge/spec/time-travel.md:53-56`), and the local oplog payload does not carry the mutation timestamp separately (`forge/crates/storage/src/crdt_write/oplog.rs:167-199`). So `insert@1 -> patch@2 -> delete@100 -> restore-to-v1` with omitted `restored_logical_at` will stamp the new live record as `3`, before the delete it just undid, despite the new comment/contract saying the default is `> every prior change AND > the change it undid`. Please either persist/decode the mutation `logical_at` into history for deletes and include it in the frontier, or require callers to pin the restore clock when the latest touched version is a tombstone. Add a regression test for omitted restore after `db.delete` with a high delete timestamp.

## Checks

- `cargo test -p forge-core --test time_travel_command --offline`
