# Review 003: forge-storage SQLite substrate

Date: 2026-06-12

Reviewed commit:

- `aa01cf1` forge-storage: SQLite KV/oplog substrate + records projection

## Summary

Nice progress: the native storage substrate has good coverage and `cargo test --locked` is green locally. The main risks are architectural: one CRDT path currently mutates append-only history, and the full workspace still cannot pass the M0a WASM lane.

## Findings

### [P1] `put_chunk` can overwrite CRDT history even though chunks are described as append-only

The module docs call `crdt_chunks`/`crdt_snapshots` the append-only rebuild source at `forge/crates/storage/src/lib.rs:11-13`, and `put_chunk` is documented as appending a CRDT op chunk at `forge/crates/storage/src/lib.rs:380`.

But the SQL at `forge/crates/storage/src/lib.rs:390-395` uses `ON CONFLICT(doc_id, chunk_id) DO UPDATE`, replacing `format`, `payload`, and `created_at` for an existing chunk id. That means a duplicate chunk id can silently rewrite the sync/replay source of truth.

For CRDT updates, prefer the stricter oplog behavior already used at `forge/crates/storage/src/lib.rs:326-347`: duplicate ids should return `StorageError` unless a future compaction path explicitly proves and records an immutable replacement.

### [P1] Full workspace still fails the M0a WASM check

Native tests pass, and `forge-domain` alone is wasm-clean. But the full workspace still fails:

```text
rquickjs-sys@0.12.0: rquickjs probably doesn't ship bindings for platform wasm32-unknown-unknown
libregexp.c:24:10: fatal error: 'stdlib.h' file not found
sqlite-wasm-rs@0.5.5: unable to create target: No available targets are compatible with triple "wasm32-unknown-unknown"
```

The storage crate still has native `rusqlite` unconditionally at `forge/crates/storage/Cargo.toml:9`, and runtime has the same unconditional native QuickJS problem noted in review 002. Since M0a’s central proof includes WASM, split storage/runtime backends with target-specific features before treating the workspace as M0a-ready.

### [P2] The transaction API does not compose typed writes yet

`Store::transact` exposes only `&rusqlite::Transaction` to the closure at `forge/crates/storage/src/lib.rs:163-166`. That proves rollback works, but it pushes future callers toward raw SQL if they need `append_op + put_chunk + put_record` in one DL-4 transaction.

Consider adding a typed `StoreTx` wrapper with transaction-scoped versions of `kv_set`, `put_record`, `append_op`, `put_chunk`, and `save_run`. That keeps the PRD promise of one transaction without making higher layers bypass the storage API.

## Verification

- `cargo test --locked`: passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-domain`: passed.
- `cargo check --locked --target wasm32-unknown-unknown`: failed on `rquickjs-sys` and `sqlite-wasm-rs` as noted above.
