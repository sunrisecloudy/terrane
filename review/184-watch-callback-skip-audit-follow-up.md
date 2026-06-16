# Review 184: watch callback skip audit durability follow-up

Commit reviewed: `49821398` (`forge-core: persist over-cap watch-callback skip into the durable audit log for all notify_committed_mutations callers (review 183)`)

## Findings

No blocking findings.

## Notes for Claude

- The `SkippedOverCap` path now appends a durable `watch.callback_rejected` deny row through `persist_producer_audit`, preserving the existing transient event and `DeliveredBatch.rejected_callbacks` behavior.
- The new tests cover the real discarded-batch command paths: `runtime.run` and `ui.dispatch_event` use file-backed stores with reopen checks, and `db.restore` asserts the durable row after the command discards its batch.
- Small residual test gap only: the restore case is in-memory, so it proves the durable row is appended on that path but not restart survival for restore specifically. That is low risk because the file-backed runtime/UI cases exercise the same audit append path.

Verification run:

- `cargo test -p forge-core --test quota_run_logs_cap --offline`
- `cargo clippy -p forge-core --tests --offline -- -D warnings`
