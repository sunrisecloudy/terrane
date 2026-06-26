# Commit Review: 73bb1248

Reviewed commit: `73bb1248 forge-core: owner-scope db.watch registration so one applet cannot hijack another's watch id (DL-16 review 134)`

The direct `db.watch` command path now rejects a cross-owner `watch_id` collision. One runtime-path gap remains.

## Finding

### P1 - `ctx.db.watch` collisions are silently accepted by the applet and only dropped after the run

The runtime host call still reports success before ownership is checked. `HostContext::db_watch` records `db.watch` as a successful host call and returns the bridge response (`forge/crates/runtime/src/host/db.rs:236`, `:245`). The core bridge only validates the query, pushes a `WatchIntent`, and returns `Ok(watch_id)` (`forge/crates/core/src/bridge.rs:666`). Ownership is not checked until `apply_watch_intents`, after the JS call has already returned and the run record has been saved (`forge/crates/core/src/commands/runtime_run.rs:182`, `:194`). On collision, `apply_watch_intents` now emits `db.watch.denied` but deliberately does not abort the run or rewrite the host-call result (`forge/crates/core/src/commands/watch.rs:682`, `:696`).

Impact: applet B can call `await ctx.db.watch("watch:open", ...)` using applet A's id. The subscription is not hijacked, which is good, but B sees a successful return value and replay records a successful `db.watch`; no active watch is actually registered for B. That contradicts the runtime-side contract that a denied watch is recorded as the run's denial and registers nothing, and it leaves applet state/UI believing it subscribed when it did not.

Suggested fix: make owner collision detectable at host-call time for live runtime runs, so `ctx.db.watch` returns `PermissionDenied` through the normal recorded-denial path instead of a late no-op. One way is to inject enough watch-session ownership context into `StorageHostBridge::db_watch` (or expose a registration validator) before recording the host call. Add a runtime regression where app A registers `watch:open`, app B's `main` attempts `ctx.db.watch("watch:open", ...)`, and B's run fails/records the denial while A's subscription remains intact.

## Verification

- `cargo test -p forge-core --test live_query_callback`
