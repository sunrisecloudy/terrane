# Commit Review: 1e7bd792

Reviewed commit: `1e7bd792 forge-core: wire db.watch notifications into the live applet ctx.db spine + next-turn callback queue (DL-16 review)`

This appears to be the amended version of the previous live-query commit. The pre-write snapshot issue from review 133 looks addressed in this committed version. Line references below are for `1e7bd792`, not the current dirty worktree.

## Finding

### P1 - `db.watch` can still hijack another applet's subscription by reusing its watch id

`db.unwatch` is now owner-scoped, but registration still treats `watch_id` as globally unique across applets. `cmd_db_watch` accepts the applet-provided id and registers it directly (`forge/crates/core/src/commands/watch.rs:98`, `:141`, `:145`). Runtime watch intents do the same (`forge/crates/core/src/commands/watch.rs:663`, `:671`). `WatchSessions::register` then replaces the first existing subscription with the same raw id, without checking the owner (`forge/crates/core/src/watch.rs:90`, `:93`).

Impact: applet B can call `ctx.db.watch("watch:tasks-open", ...)` with applet A's visible id and replace A's owner/callback/query. A's later `db.unwatch` will no longer cancel its own subscription because the owner has been changed to B, and notifications now route through B's callback/query. This is the same capability ownership boundary that review 132 fixed for `unwatch`, just on the registration path.

Suggested fix: store sessions and the storage registry under an internal composite/global id such as `(applet_id, local_watch_id)` while returning the local id to the applet, or reject cross-owner reuse of an existing local id. Please add a two-applet regression where app B calls `ctx.db.watch` using app A's watch id and app A's subscription remains intact. The current test only covers app B attempting `db.unwatch` (`forge/crates/core/tests/live_query_callback.rs:291`).

## Verification

Not run for this review pass: the working tree already has uncommitted follow-up edits in the same live-query files, so a local test run would not verify the committed `1e7bd792` state exactly.
