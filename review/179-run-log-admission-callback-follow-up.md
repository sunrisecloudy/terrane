# Review 179 - Run-log admission callback follow-up (18039e73)

Findings:

- [P1] Do not bubble callback admission failure back as a producer command failure after the producer already committed. `runtime.run` now admits the producer before execution, then commits its run record at `forge/crates/core/src/commands/runtime_run.rs:260-279`, and only afterwards delivers live-query notifications at `forge/crates/core/src/commands/runtime_run.rs:299-305`. If that save pushes `run_logs` to the cap, `dispatch_notification_callback` rejects the watcher callback at `forge/crates/core/src/commands/watch.rs:568-578`; because `run_notify_turn` uses `?` on that callback result (`forge/crates/core/src/commands/watch.rs:501-524`), the `ResourceLimitExceeded` bubbles out of the producer `runtime.run` even though the producer's record writes and run record are already durable. The new test currently codifies the same behavior for direct `commit_and_notify`: it expects an error after the triggering mutation has already committed (`forge/crates/core/tests/quota_run_logs_cap.rs:251-269`). That leaves callers seeing a failed write/run while the producer side effects did land. Treat callback admission denial as a skipped/deferred callback delivery (record/emit a callback-rejected notification, maybe with retry/backpressure), or otherwise keep the producer response successful once its own durable effects and run record have committed.

- [P2] Count the producer dispatch run before admitting callback runs, or the cap can overshoot by more than the documented one record. The spec now says admission can let `run_logs` go "up to one record past the cap" before the next run is rejected (`forge/spec/quotas.md:207-211`). `ui.dispatch_event` runs handler writes, delivers notifications, and can admit/save watcher callback runs before assigning/saving the dispatch run record (`forge/crates/core/src/commands/ui.rs:394-400`, `forge/crates/core/src/commands/ui.rs:479-491`). With one byte of headroom, the dispatch admission passes, a watcher callback admission also sees the old pre-dispatch `run_logs` value and passes, the callback run record lands, and then the dispatch run record lands too: two records beyond the cap. Save/count the dispatch run before notification callback admission, or make callback admission reserve against the already-admitted producer record. Add a near-cap UI-dispatch-with-watch fixture so the "one record past cap" invariant is pinned.

Checks:

- `cargo test -p forge-core --test quota_run_logs_cap --offline`
- `cargo test -p forge-core --test quota_core_conformance --offline`
- `cargo test -p forge-core --test live_query_callback --offline`
- `cargo test -p forge-storage --test quota_fixtures --offline`
