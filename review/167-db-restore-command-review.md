# Review: 75ca1b31 db.history + db.restore commands

## Findings

- **P1 - `db.restore` bypasses live-query notifications.** `cmd_db_restore` calls `self.store.restore_record(...)` and returns the new state directly (`forge/crates/core/src/commands/time_travel.rs:133-152`), but never snapshots the watch registry or runs `notify_committed_mutations` / `commit_and_notify`. DL-16 says a watch observes the projection after each committed mutation transaction, and insert/update/patch/delete dirty their target id (`forge/spec/live-queries.md:40-62`). A restore is implemented as a new `record.insert` or `record.delete`, so any active `db.watch` over that collection will stay stale after an undo/restore. Please route restore through the same notification path used for already-committed `ctx.db` writes (`forge/crates/core/src/commands/watch.rs:400-449`) or add a small helper that snapshots before restore, records the actual mutation kind/id, and drives the notification turn after the storage transaction commits.

- **P2 - Omitted `restored_logical_at` can move the restored record's WHEN backwards.** When the caller omits the timestamp, the command stamps the restore from `EventSink` (`forge/crates/core/src/commands/time_travel.rs:114-129`), whose counter starts independently at 0 and increments on events (`forge/crates/core/src/event.rs:41-49`). The CRDT restore path then writes an `Insert` with that exact timestamp as the new envelope's `updated_at` (`forge/crates/storage/src/crdt_write/mutation.rs:49-60`). In the new `live_db_restore_without_pinned_clock_is_replay_safe` scenario, the seeded record has logical times 1 and 2, but the omitted restore gets EventSink time 1; the new v3 history row therefore reports `logical_at=1`, colliding with the original insert and earlier than the change it undid. That contradicts the time-travel spec's monotone restore timestamp contract (`forge/spec/time-travel.md:97-99`) and makes audit/undo ordering misleading. Consider deriving the default from the data frontier/current record clock (or requiring the caller to pass it) and assert the default restore entry is greater than the prior record timestamp, not merely non-null.

## Checks

- `cargo test -p forge-core --test time_travel_command --offline`
- `cargo test -p forge-storage retained_change_feed_survives_compaction_with_state_intact --offline`
