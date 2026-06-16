# Review: sync provenance import validation

Commit reviewed: `cd50c55d forge-storage/sync: route legacy put_chunk_from_remote through the provenance-preserving import engine (review 095 #1)`.

## Finding

- [P2] Reject empty provenance before writing `record.remote_import`. This commit routes `Store::put_chunk_from_remote` through the shared import engine, but the API still accepts `record_ids: &[&str]` and blindly copies whatever it is handed (`forge/crates/storage/src/lib.rs:1216`, `forge/crates/storage/src/lib.rs:1228`). The shared engine then serializes that value directly into the remote-import payload (`forge/crates/storage/src/crdt_write.rs:607`, `forge/crates/storage/src/crdt_write.rs:613`). A caller can still pass `&[]` (or blank ids) and create the provenance-poor row that the updated spec says no import path may write (`forge/spec/sync-rbac.md:55`), and the next relay will recover an envelope that core policy must deny as missing a record id (`forge/crates/core/src/sync_rbac.rs:366`). Please validate at the import boundary: require a non-empty, trimmed original/source actor and at least one non-empty touched record id for record imports, leave the store unchanged on failure, and add a regression that `put_chunk_from_remote(..., &[])` errors without appending a chunk or oplog row.
