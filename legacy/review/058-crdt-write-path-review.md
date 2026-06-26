# Review: `50a30338` CRDT-backed storage write path

Findings for Claude:

- **P1: The commit depends on uncommitted T024 fixtures.** `forge/crates/storage/src/crdt_write.rs:863` loads `../../fixtures/crdt-write/*.json`, but `git show --name-only 50a30338` includes only storage/Cargo files and does not include `forge/fixtures/crdt-write/` or `forge/spec/crdt-write-path.md`. The current dirty worktree has those files, so `cargo test --locked -p forge-storage crdt_write` passes locally, but a clean checkout of this commit will fail the new fixture tests. Add the fixtures/spec to the commit or remove the fixture-backed tests until the corpus is committed.

- **P1: The applet-facing spine still bypasses the new CRDT/oplog path.** `forge/crates/core/src/bridge.rs:171-197` still handles `ctx.db.insert` by building a `RecordEnvelope` and calling `self.store.put_record(&env)`, so real applet writes do not append `crdt_chunks` or `oplog` rows. Since `Store::rebuild_projection()` now deletes `records` and rebuilds only from `crdt_chunks` (`forge/crates/storage/src/crdt_write.rs:456-488`), running rebuild after normal spine writes would drop those records. Wire the core bridge to `apply_mutation_crdt`/`transact_mutations_crdt` before treating DL-4/DL-6 as covered, or gate rebuild away from projection-only records.

- **P2: Delete is implemented as hard removal, not the default tombstone model.** The CRDT path calls `doc.delete_record(id)` (`forge/crates/storage/src/crdt_write.rs:253-263`) and materialization deletes the projection row (`forge/crates/storage/src/crdt_write.rs:510-518`). That means `include_deleted`, data-browser views, and future audit/change-feed surfaces cannot see a deleted envelope after delete/rebuild. `prd-merged/02-data-layer-prd.md:55` includes `deleted` in every envelope, and DL-21 says deletion is tombstone-by-default with hard-purge only for explicit purge classes. Keep a `deleted=true` envelope/tombstone in CRDT history/projection, or add the `tombstones` path before using whole-record removal.

- **P2: Oplog lamports are per collection, not workspace-global.** `append_op_tx` receives `chunk_id_lamport(&chunk_id)` (`forge/crates/storage/src/crdt_write.rs:425-435`), and chunk ids restart at `chunk-0001` for each `collection/<name>` doc. `list_ops()` orders by `(lamport, op_id)` (`forge/crates/storage/src/lib.rs:1059-1067`), so writes to different collections can share lamport `1` and replay in lexical `op_id` order instead of actual write order. Use a workspace-level monotone counter/HLC for oplog lamport while keeping chunk ids per doc.

Verification:

- `cargo test --locked -p forge-storage crdt_write` passes in the current dirty worktree because the untracked T024 fixtures are present.
