# Review: 3d1c9b7e DL-20 time travel + retention

## Findings

- **P1 - Retained change-feed entries reconstruct as tombstones after compaction.** `record_history()` accumulates chunks in `get_chunks()` order and reconstructs a retained op from the chunks seen so far (`forge/crates/storage/src/time_travel.rs:99-123`). After retention compaction, `compact_doc_tx()` writes `compact-NNNN` with a fresh `created_at` after the retained suffix chunks (`forge/crates/storage/src/compaction.rs:245-297`), while `get_chunks()` orders by `(created_at, chunk_id)` (`forge/crates/storage/src/crdt.rs:221-228`). That leaves storage ordered like `chunk-0004`, `chunk-0005`, `compact-0003`; the feed entries for v4/v5 are still present, but their replay prefix does not include the compact base yet, so the feed reports `state=None` and `logical_at=None` for live retained changes. This breaks DL-20's retained 90-day who/when/what feed and undo/audit surface even though the retention tests pass because they only assert oplog row presence. Please reconstruct each entry from all chunks with frontier `<= version` (or sort compact snapshots before their retained suffix for replay) and add a retention fixture that checks `record_history()` after compaction returns v4/v5 with titles/logical_at intact.

## Checks

- `cargo test -p forge-storage --test time_travel_fixtures --offline`
- `cargo test -p forge-storage retention_window_keeps_within_window_change_feed_and_prunes_beyond --offline`
- Throwaway `/private/tmp` harness reproduced `chunks=["chunk-0004","chunk-0005","compact-0003"]` and `history v4/v5 ... at=None title=None` after `RetentionPolicy::new(2)`.
