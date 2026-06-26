# Commit Review: 6de7775b CRDT transact group single chunk

Reviewed commit: `6de7775b forge-crdt: fix DL-4 transact-group primitive — pin single-commit multi-mutation export to one rebuildable chunk (DL-4/DL-6) — 36 tests`

## Findings

- No new findings. The commit adds a focused regression proving that multiple record mutations applied before one `RecordsDoc::commit()` export as one update chunk and rebuild to the same materialized document through `RecordsDoc::from_updates`.

## Notes

- This matches the T024 `transact_group_single_chunk` fixture intent: storage can capture one pre-group version, apply child mutations, commit once, export one chunk, and rebuild the whole group from that chunk.
- The diff is test-only in `forge/crates/crdt/src/lib.rs`; no production API or behavior changes were introduced by this commit.

## Verification

- `git show --check 6de7775b`
- `cargo test --locked -p forge-crdt transact_group_of_mutations_exports_one_chunk_that_rebuilds_whole_group`
