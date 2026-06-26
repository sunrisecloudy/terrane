# Commit Review: 82c15b8c runtime db.query host call

Reviewed commit: `82c15b8c forge-runtime: ctx.db.query host call (DL-15)`

## Findings

- **P1 - Clean `forge-core` no longer implements the `HostBridge` trait.** The commit adds required `HostBridge::db_query` at `forge/crates/runtime/src/bridge.rs:49`, but the committed `StorageHostBridge` impl in `forge/crates/core/src/bridge.rs:106` still only implements `db_insert`/`db_get`/`db_list` and then jumps to `ui_render` at `forge/crates/core/src/bridge.rs:183`. A clean checkout of this commit will fail to build any target that checks `forge-core` with a missing trait item. Please add the real core bridge implementation in the committed tree, parse the structured `Query`, force `q.from = collection`, run `Store::query`, and serialize rows/aggregate/groups consistently with the runtime contract.

- **P2 - The in-memory bridge widens reads using caller-supplied `query.from`.** `HostContext::db_query` checks `db.read` only for the trusted `collection` argument (`forge/crates/runtime/src/host.rs:273`), but `MemoryHostBridge::db_query` chooses the collection from `query.from` first (`forge/crates/runtime/src/bridge.rs:162`). That means a test applet can call `ctx.db.query("tasks", { "from": "users" })` and the memory bridge reads `users` after policy only approved `tasks`. The real-store test bridge pins `q.from = collection` (`forge/crates/runtime/tests/storage_bridge.rs:99`), so the test double should do the same, or `HostContext` should normalize the query before passing it to any bridge.

## Verification

- `git show --check 82c15b8c`
- Inspected the committed tree with `git show HEAD:forge/crates/core/src/bridge.rs` and `git show HEAD:forge/crates/runtime/src/bridge.rs` so local dirty bridge changes did not mask the missing implementation.
