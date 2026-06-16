# Review 152 - sync audit deny-only rebuild

Commit reviewed: `55d9fc9b forge-core: persist sync-RBAC audit rows in the same txn as the import they record (review 149)`

## Finding

- [P2] Denied-only sync still rebuilds the receiver projection. `Store::apply_remote_chunks_with_audit` skips the transaction only when both `chunks` and `audit_rows` are empty, then always calls `rebuild_projection_tx` before appending audit rows (`forge/crates/storage/src/crdt_write/remote.rs:71-99`). But `sync_stores_authorized` passes exactly this shape for rejected chunks: empty `allowed_to_*` plus non-empty deny audit rows (`forge/crates/sync/src/lib.rs:662-687`). The sync-RBAC spec says a rejection must skip import and leave local projections unchanged, and only after checks pass should the chunk be imported and projection rebuilt (`forge/spec/sync-rbac.md:108-113`). Please special-case `chunks.is_empty()` to append audit rows only, without projection/index rebuild DML, and add a regression that a denied-only sync records the audit row while leaving receiver records/indexes byte-for-byte unchanged.

