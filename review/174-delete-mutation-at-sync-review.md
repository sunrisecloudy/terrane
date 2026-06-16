# Review: 9178f538 delete mutation_at sync boundary

## Findings
- No actionable findings. The commit threads a delete chunk's `mutation_at` from
  the origin oplog through `SyncOpEnvelope` / `RemoteChunk` into the receiver's
  `record.remote_import` oplog row, so DL-20 history and omitted restore clocks
  count a synced late delete instead of defaulting before it.

## Verification note
- The first targeted `forge-sync` run replayed stale `target/` artifacts and
  failed with an unused-variable warning. After `cargo clean -p forge-storage -p
  forge-sync`, the fresh build passed.

## Checks
- `cargo test -p forge-sync synced_late_delete_carries_mutation_at_so_receiver_restore_clock_exceeds_it --offline`
- `cargo test -p forge-sync --offline`
- `cargo test -p forge-storage delete_version_reports --offline`
- `cargo test -p forge-core --test time_travel_command --offline`
