# Review 122: storage facade split

Reviewed commit `5be46e57` (`forge-storage: split lib.rs into per-concern modules behind a re-export facade`).

## Findings

- No blocking findings. `forge/crates/storage/src/lib.rs` now acts as a re-export facade while the existing `Store` accessors move into per-concern modules (`store`, `kv`, `records`, `records_indexed`, `mutations`, `oplog`, `crdt`, `runs`, `query_exec`), and the public types used by downstream crates remain re-exported from the crate root.
- The split keeps the high-risk storage invariants covered by existing tests: atomic CRDT writes and grouped mutations, KV tombstones/listing/counters, query planning/index use, export/import consistency, run record validation, compaction, and FTS sync.

## Verification

- `cargo test -p forge-storage`
- `cargo clippy -p forge-storage -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
