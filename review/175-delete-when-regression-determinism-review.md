# Review: cf8c3e25 delete-WHEN regression determinism

## Findings
- No actionable findings. The commit is test-only and makes the DL-20 review 171
  regression deterministic by splitting the proof into a direct `RemoteChunk`
  import seam check and an order-independent end-to-end `sync_stores` check. This
  keeps the delete `mutation_at` coverage load-bearing without depending on
  content-addressed chunk ordering or `record_history` version ordering.

## Checks
- `cargo test -p forge-sync synced_late_delete_carries_mutation_at_so_receiver_restore_clock_exceeds_it --offline`
- `cargo test -p forge-sync --offline`
