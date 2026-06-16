# Commit Review: 07e8c472 core db.query host call

Reviewed commit: `07e8c472 forge-core: ctx.db.query host call (DL-15)`

## Findings

- **P1 - New denied-query spine test depends on an uncommitted runtime API shape.** The committed runtime still installs only `ctx.db.query(collection, query)` (`forge/crates/runtime/src/engine.rs:592`), but the new denied test calls `ctx.db.query({ from: "secrets", limit: 1 })` at `forge/crates/core/tests/spine.rs:1150` and expects the host policy denial path (`CapabilityRequired`). On a clean tree for this commit, the QuickJS host function receives the plan object as the `collection` argument and an absent second argument, so it fails before the policy check as a runtime/marshalling error instead of recording `db.query`. Please either commit the matching runtime overload for `query(plan)` or change this test/applet to call `ctx.db.query("secrets", { from: "secrets", limit: 1 })` until the one-argument std surface lands.

## Notes

- The real `StorageHostBridge::db_query` implementation correctly pins `q.from = collection` before calling storage (`forge/crates/core/src/bridge.rs:230`), which closes the clean-tree missing-impl issue from review 052.
- The in-memory runtime bridge still reads caller-supplied `query.from` (`forge/crates/runtime/src/bridge.rs:162`), so the review 052 test-double grant-widening concern remains open outside this core-only patch.

## Verification

- `git show --check 07e8c472`
- Inspected committed `forge/crates/runtime/src/engine.rs`, `forge/crates/core/src/bridge.rs`, and `forge/crates/core/tests/spine.rs` via `git show HEAD:<path>` so local uncommitted runtime overload work did not mask the API mismatch.
