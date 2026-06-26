# Review: commit 841567a9 live-query core/runtime wiring

Claude, I found two concrete follow-ups in the new `ctx.db.watch` / callback dispatch wiring.

## Findings

1. **P1: Real watch-callback `ctx.db` writes do not produce the required next notification turn.**
   - In `WorkspaceCore::commit_and_notify`, the dirty set/notifications are computed only from the original `mutation` before callback dispatch (`forge/crates/core/src/commands/watch.rs:320`, `forge/crates/core/src/commands/watch.rs:344`).
   - The callback is then run with a live `StorageHostBridge` (`forge/crates/core/src/commands/watch.rs:442`), whose `db_insert` writes directly to SQLite/CRDT via `apply_mutation_crdt` (`forge/crates/core/src/bridge.rs:320`, `forge/crates/core/src/bridge.rs:353`).
   - After that, `dispatch_notification_callback` explicitly returns no queued mutation because the writes "already committed" (`forge/crates/core/src/commands/watch.rs:475`, `forge/crates/core/src/commands/watch.rs:485`). There is no later `commit_and_notify` call for those already-applied writes, so other watches never receive the required next-turn notification/version.
   - This contradicts the T047 fixture decision that callback mutations are queued as the next event-loop turn. The current facade test at `forge/crates/core/tests/live_query_e2e.rs:473` models the callback effect by manually calling `commit_and_notify` a second time, so it does not cover a real `onWatch` handler that calls `ctx.db.insert`/patch.
   - Suggested fix: make notification-callback `ctx.db` mutations queue into the facade turn loop instead of applying silently outside watch delivery, or capture the applied mutations/dirty changes and drive follow-up `commit_and_notify` turns. Add a real callback test where `onWatch` mutates a watched collection and assert a second notification with a later version.

2. **P1: `db.unwatch` is not scoped to the owning applet, so one applet can cancel another applet's live query.**
   - The runtime `ctx.db.unwatch(watch_id)` path is deliberately not policy-gated (`forge/crates/runtime/src/host/db.rs:154`, `forge/crates/runtime/src/host/db.rs:171`).
   - The bridge records only `WatchIntent::Unwatch { watch_id }` (`forge/crates/core/src/bridge.rs:451`, `forge/crates/core/src/bridge.rs:455`).
   - `apply_watch_intents(applet_id, ...)` receives the caller's applet id but ignores it for unwatch and removes the raw id (`forge/crates/core/src/commands/watch.rs:493`, `forge/crates/core/src/commands/watch.rs:519`); the command path does the same (`forge/crates/core/src/commands/watch.rs:184`, `forge/crates/core/src/commands/watch.rs:186`). `WatchSessions::unregister` also retains only by `watch_id`, not owner (`forge/crates/core/src/watch.rs:100`, `forge/crates/core/src/watch.rs:104`).
   - Since watch ids are applet-visible strings, any runnable applet that guesses/learns another id can stop that subscription. That breaks the CR-3 capability-scoped host API model and the subscription ownership implied by `subscription.unwatch()`.
   - Suggested fix: make unregister owner-aware for applet-originated intents (`watch_id` + owning `applet_id` must match), and decide whether command-level unwatch is owner/admin-only or explicitly applet-scoped. Add a two-applet regression test where applet B attempts to unwatch applet A's watch and A still receives notifications.

## Verification

- `cargo test -p forge-core --test live_query_callback --test live_query_e2e`
- `cargo test -p forge-runtime --lib`
- `cargo clippy -p forge-core --tests -- -D warnings`
- `cargo clippy -p forge-runtime --tests -- -D warnings`
