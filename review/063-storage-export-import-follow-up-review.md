# Review 063: storage export/import follow-up (`670041ae`)

Claude, this commit usefully closes the exact-root namespace leak and makes destination writes rollback together. The remaining blocker is that the source side of export/import is still not a coherent DL-24 snapshot.

## Findings

- **P1: export is still not source-snapshot consistent.** `write_bundle` now wraps writes to the destination bundle in one transaction (`forge/crates/storage/src/export.rs:227`), but it still reads the source through separate table scans (`copy_kv(&self.conn, ...)`, `copy_oplog(&self.conn, ...)`, `copy_crdt_chunks(&self.conn, ...)`, `copy_records(&self.conn, ...)` at `forge/crates/storage/src/export.rs:230`). Each helper prepares its own `SELECT` against `self.conn` (for example `copy_kv` at `forge/crates/storage/src/export.rs:426`, `copy_oplog` at `forge/crates/storage/src/export.rs:468`, and `copy_records` at `forge/crates/storage/src/export.rs:573`). A second file-backed writer can commit between those scans, leaving a bundle whose KV/oplog/chunks/records come from different moments. Please hold a read transaction/snapshot on the source for the full copy and add a concurrent-writer regression.

- **P1: import still drops projection-only records.** Export copies `records` into the bundle (`forge/crates/storage/src/export.rs:236`), but import deliberately skips bundled records and rebuilds only from CRDT chunks (`forge/crates/storage/src/export.rs:306`). `Store::put_record` remains a public projection API that can create records without chunks (`forge/crates/storage/src/lib.rs:450`), so those rows are exported but disappear after import. DL-24 requires re-import to reproduce a byte-identical projection (`prd-merged/02-data-layer-prd.md:83`). Either migrate all remaining writers to chunk-backed writes before declaring this safe, or reject/export-flag projection rows that lack CRDT source.

- **P1: snapshot-only/compacted CRDT history still cannot rebuild.** The bundle copies `crdt_snapshots`, but `rebuild_projection` discovers docs only from `crdt_chunks` (`forge/crates/storage/src/crdt_write.rs:461`) and `load_doc_tx` only loads chunk payloads (`forge/crates/storage/src/crdt_write.rs:154`). Once DL-19 compaction folds history into snapshots, a valid compacted export can import with missing records. Please teach rebuild/import to seed from snapshots or add a fixture that proves snapshot-only exports are rejected rather than silently losing data.

## Verification

- `cargo test --locked -p forge-storage export`
- `cargo test --locked -p forge-storage local_only_namespace_policy_is_precise`
- `cargo test --locked -p forge-storage import_round_trips_through_real_files`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
