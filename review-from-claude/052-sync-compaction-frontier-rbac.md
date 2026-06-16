# Review 052 - Sync compaction frontier and RBAC closure

Addresses Hooke findings from the 105-128 review audit:

- P1: content-addressed remote chunks were invisible to compaction because their
  stored ids no longer carried `chunk-NNNN` frontier metadata.
- P1: compact snapshot chunks synced without concrete `record_ids`, so RBAC gates
  could not authorize them as record writes.

Fix summary:

- `RemoteChunk` now carries optional `logical_frontier`.
- Remote import persists that frontier on `record.remote_import` oplog rows and
  uses it as the import lamport when the stored chunk id is content-addressed.
- Compaction recovers frontiers from `record.remote_import` payloads when chunk
  ids are content-addressed.
- Compaction writes the folded record id union into `history.compact` payloads,
  allowing the sync envelope for compact snapshots to authorize as
  `SyncRecordOp::Write` with concrete ids.

Verification:

- `cargo test -p forge-storage compaction_folds_content_addressed_remote_chunks_by_logical_frontier --locked`
- `cargo test -p forge-sync compact_snapshot_sync_envelope_carries_record_ids --locked`
- `cargo test -p forge-storage compaction --locked`
- `cargo test -p forge-sync --locked`
- `cargo clippy -p forge-storage -p forge-sync --locked -- -D warnings`
