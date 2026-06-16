# Commit Review: 5b04d975

Reviewed commit: `5b04d975 forge-core: wire db.watch notifications into the live applet ctx.db spine + next-turn callback queue (DL-16 review)`

Line references below are for the committed diff, not the current dirty worktree. I also see an uncommitted `forge/crates/core/src/bridge.rs` edit that appears to be exploring part of the first fix; this review is still flagging what landed in `5b04d975`.

## Findings

### P1 - Live `ctx.db` notifications snapshot after the write, so leave/update/delete cases are missed

`StorageHostBridge` records only bare mutations after applying them (`forge/crates/core/src/bridge.rs:440`, `:455`, `:480`, `:501`, `:515`). `notify_committed_mutations` then takes the watch snapshot later, after the live applet write already landed (`forge/crates/core/src/commands/watch.rs:389`, `:398`), and callback-drained writes do the same (`forge/crates/core/src/commands/watch.rs:427`). That violates the storage watch contract: snapshots are intended to capture result ids before mutation (`forge/crates/storage/src/watch.rs:329`, `:461`) so commit can compare before/after membership (`forge/crates/storage/src/watch.rs:513`).

Impact: inserts notify because the dirty id is present in the post-write result set, but `ctx.db.patch`/`ctx.db.delete` that make a record leave a watched query can be suppressed because both `before_ids` and `after_ids` are computed after the write. A multi-write live applet run can also deliver earlier notification turns against the final store state instead of the state after each transaction.

Suggested fix: capture `ResultSnapshot` before each live `ctx.db` write and pass `(mutation, before_snapshot)` into `run_notify_turn`, or otherwise replay notification evaluation against per-transaction before states. Please add regression tests for a watched record leaving via `ctx.db.patch`/`ctx.db.delete`, plus a multi-write run where notifications preserve transaction order.

### P1 - `db.watch` can still replace another applet's watch by reusing the same local id

The unwatch path is now owner-scoped, but registration is still keyed only by raw `watch_id`. `cmd_db_watch` accepts the caller-provided id and registers it directly (`forge/crates/core/src/commands/watch.rs:82`, `:130`), while `WatchSessions::register` replaces any existing subscription with the same id regardless of owner (`forge/crates/core/src/watch.rs:90`). Runtime watch intents also register the applet-provided id directly (`forge/crates/core/src/commands/watch.rs:650`).

Impact: applet B can call `ctx.db.watch("watch:open", ...)` with applet A's visible watch id and silently replace A's owner/callback/query. After that, A's `db.unwatch` no longer removes the subscription because ownership has been changed, and notifications route through B's subscription instead.

Suggested fix: key active watch sessions/registry entries by `(applet_id, local_watch_id)` or mint an unguessable internal id while preserving the applet-local id in the API. Please add a two-applet regression where B reuses A's watch id and A's subscription remains intact.

## Verification

Focused checks passed before the current uncommitted follow-up edit appeared:

- `cargo test -p forge-core --test live_query_callback --test live_query_e2e --test live_query_spine`
- `cargo test -p forge-runtime --lib`
- `cargo clippy -p forge-core --tests -- -D warnings`
- `cargo clippy -p forge-runtime --tests -- -D warnings`
