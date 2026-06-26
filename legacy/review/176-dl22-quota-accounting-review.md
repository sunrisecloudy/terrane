# Review: 3eed9654 DL-22 quota accounting

## Findings
- [P1] The live records-write gate under-projects the bytes that `quota_usage`
  will report after commit. `quota_usage` defines `workspace_total_bytes` as
  per-applet `records.data` bytes plus every category, including `crdt_chunks`
  and `oplog` metadata (`forge/crates/storage/src/quota.rs:343-421`), matching
  the new spec table in `forge/spec/quotas.md:39-48`. But the real write path
  checks only `chunk_payload.len()` before it appends the chunk, appends the
  oplog row, and materializes the projection
  (`forge/crates/storage/src/crdt_write/mutation.rs:363-390`), and
  `decide_quota` adds that same single `write_bytes` to workspace, retained
  chunks, and the per-applet collection budget
  (`forge/crates/storage/src/quota.rs:557-575`). A write can therefore pass when
  `before_total + chunk_bytes <= workspace_limit` but then commit additional
  accounted `records` and `oplog` bytes, leaving the workspace over quota
  immediately after an accepted write. It also compares the per-applet
  "collections" cap against CRDT chunk bytes rather than the projected
  `records.data` delta. Please charge the same slices the report counts (or
  stage the write, recompute `quota_usage` inside the transaction, and roll back
  on overflow), then add fixtures proving accepted writes do not leave
  workspace/per-applet/cache usage over their limits.
- [P2] `put_attachment` performs dedup lookup, quota check, and insert/update as
  separate autocommit statements (`forge/crates/storage/src/quota.rs:649-681`).
  With two file-backed `Store` handles, both writers can observe the same
  pre-write headroom and then insert distinct blobs that together exceed the
  attachments/workspace cap; two identical first puts can also race into a
  primary-key error instead of one insert plus one refcount bump. Wrap this path
  in a single transaction that takes the writer lock before the lookup/check and
  uses an insert-or-update/upsert shape, then add a two-handle regression around
  quota oversubscription and duplicate-byte races.

## Checks
- `cargo test -p forge-storage --test quota_fixtures --offline`
- `cargo test -p forge-storage quota --offline`
- `cargo test -p forge-domain content_hash --offline`
