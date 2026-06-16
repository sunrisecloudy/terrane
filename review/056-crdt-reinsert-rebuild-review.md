# Commit Review: 7caa8f99 CRDT reinsert rebuild regression

Reviewed commit: `7caa8f99 forge-crdt: fix DL-4/DL-6 — harden rebuild primitive against reinsert-after-delete (35 tests)`

## Findings

- No new findings. The commit adds a targeted DL-6 regression test for `insert -> delete -> reinsert same id`, proving `RecordsDoc::from_updates` rebuilds the recreated record rather than leaving the id hidden by a stale delete marker.

## Notes

- This matches the T024 fixture intent for `reinsert_after_delete_rebuild.json` and gives the CRDT primitive direct coverage before the storage-level DL-4 orchestration lands.
- The diff is test-only in `forge/crates/crdt/src/lib.rs`; no production API or behavior changes were introduced by this commit.

## Verification

- `git show --check 7caa8f99`
- `cargo test --locked -p forge-crdt rebuild_after_reinsert_following_delete_shows_recreated_record`
