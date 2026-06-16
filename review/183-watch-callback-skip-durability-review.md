# Review: b9389051 watch callback skip durability

## Finding

- **P1 - Persist skipped callback decisions on command paths.** `run_notify_turn` records an over-cap callback skip only into `DeliveredBatch.rejected_callbacks` and emits a transient `db.watch.callback_rejected` event (`forge/crates/core/src/commands/watch.rs:80`, `forge/crates/core/src/commands/watch.rs:560`). That works for direct `commit_and_notify` tests because the caller keeps the returned batch, but the real `runtime.run`, `ui.dispatch_event`, and time-travel restore paths call `notify_committed_mutations(...)?` and discard the returned `DeliveredBatch` (`forge/crates/core/src/commands/runtime_run.rs:299`, `forge/crates/core/src/commands/ui.rs:418`, `forge/crates/core/src/commands/time_travel.rs:154`). Since `EventSink` is in-memory only (`forge/crates/core/src/event.rs:1`), the new `db.watch.callback_rejected` decision is lost after the command returns/restart, even though `forge/spec/quotas.md:233` says the skip is a recorded decision that replay deterministically reproduces. Please thread `rejected_callbacks` into a durable replay artifact (or a durable notification/decision log) for all `notify_committed_mutations` callers, and add a `runtime.run`/`ui.dispatch_event` regression that verifies the skip survives beyond the transient event/batch.

## Verification

- `cargo test -p forge-core --test quota_run_logs_cap --offline`
- `cargo test -p forge-core --test live_query_callback --offline`
