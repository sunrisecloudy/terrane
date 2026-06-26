# Review: sync integration commits

Commits reviewed: `34fcfbef`, `f9911eb9`, `df44f71a`

## Findings

1. **[P1] Rebuild each workspace with its own `IndexManager` during sync.**
   `WorkspaceCore::sync_with` passes only `self.indexes` into
   `forge_sync::sync_stores` (`forge/crates/core/src/workspace.rs:361-366`),
   and `sync_stores` uses that one manager for both stores' projection rebuilds
   (`forge/crates/sync/src/lib.rs:204`, `forge/crates/sync/src/lib.rs:232-234`).
   But `Store::rebuild_projection` uses the provided manager both while
   materializing records and while rebuilding physical indexes
   (`forge/crates/storage/src/crdt_write.rs:466`, `:493`, `:499`). With
   asymmetric indexes this becomes order-dependent: if the caller has an active
   FTS index the other DB does not have, the other rebuild can issue FTS DML
   against a missing table; if only `other` has active indexes, they are skipped
   and can be stale after the projection is dropped/rebuilt. Please change the
   sync API to accept per-store index managers, or have `WorkspaceCore::sync_with`
   rebuild `self.store` with `self.indexes` and `other.store` with
   `other.indexes`. Add a regression with asymmetric FTS/value indexes on the two
   cores and assert sync is not order-dependent.

2. **[P2] Remote chunk imports bypass the DL-4 oplog path.**
   The new sync path imports missing chunks with `Store::put_chunk` and then
   rebuilds projections (`forge/crates/sync/src/lib.rs:216-234`). `put_chunk`
   only inserts into `crdt_chunks` (`forge/crates/storage/src/lib.rs:1144-1167`),
   while local CRDT writes append both `crdt_chunks` and an `oplog` row in one
   logical path (`forge/crates/storage/src/crdt_write.rs:431-445`). This
   contradicts `prd-merged/02-data-layer-prd.md:49`, which says remote updates
   follow the identical `crdt_chunks` + `oplog` path. Please either add a remote
   import API that records accepted chunks in `oplog` (with remote actor/source
   metadata), or explicitly document this as a temporary M0b gap and add a
   failing/pending test so the audit/change-feed surface is not forgotten.

## Notes

- The content-addressed exchanged chunk IDs and distinct CRDT peer ids look like
  the right direction for SS-1/SS-2 and close the obvious local `chunk-0001`
  collision hazard.
- I did not run the full Cargo test suite during this heartbeat; this is a diff
  review only.
