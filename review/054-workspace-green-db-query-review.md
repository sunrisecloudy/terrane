# Commit Review: 54bc8545 workspace green db.query

Reviewed commit: `54bc8545 forge: workspace green (ctx.db.query)`

## Findings

- **P2 - Two-argument `ctx.db.query` still lets the in-memory bridge read `query.from` instead of the checked collection.** The new overload correctly derives the trusted collection from `from` for the one-argument form (`forge/crates/runtime/src/engine.rs:600`), and `HostContext` checks only the `collection` argument it passes to the bridge (`forge/crates/runtime/src/host.rs:273`). But `MemoryHostBridge::db_query` still prefers `query.from` over that already-checked `collection` (`forge/crates/runtime/src/bridge.rs:162`). A runtime/test applet can still call `ctx.db.query("tasks", { "from": "users" })`; policy checks `tasks`, then the memory bridge reads `users`. The real core bridge pins `from`, so this is a test-double contract drift that can hide grant-boundary regressions. Please either make `MemoryHostBridge` read `collection` authoritatively or normalize/pin the query before any bridge sees it, and add a mismatch test for the two-argument form.

## Notes

- The prior review 053 P1 is addressed: the committed runtime now supports the one-argument `ctx.db.query(plan)` shape used by the denied-query fixtures/tests.

## Verification

- `git show --check 54bc8545`
- Inspected committed `forge/crates/runtime/src/engine.rs`, `forge/crates/runtime/src/host.rs`, and `forge/crates/runtime/src/bridge.rs` via `git show HEAD:<path>`.
