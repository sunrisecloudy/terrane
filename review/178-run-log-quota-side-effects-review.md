# Review 178 - Run-log quota rejection ordering (a3855c63)

Findings:

- [P1] Do not reject run-record persistence after live applet effects have already committed. The new `save_run_with_quota_tx` gate is called only after `record_run_with_context` has run against the live `StorageHostBridge` (`forge/crates/core/src/commands/runtime_run.rs:150-177`), whose `ctx.db` calls immediately apply CRDT mutations to SQLite (`forge/crates/core/src/bridge.rs:598-615`). If `save_run_with_quota_tx` then returns `ResourceLimitExceeded` (`forge/crates/core/src/commands/runtime_run.rs:241-259`), the command fails without a `RunRecord`, but the applet's record writes are still durable. That violates CR-9's "every execution persists ... resulting writes" replay/audit contract (`prd-merged/01-core-runtime-prd.md:52-53`) and creates unreplayable side effects. The same ordering exists in `ui.dispatch_event`: handler writes and notifications are applied before the run-log save, and the success path also persists the new UI tree before the quota gate (`forge/crates/core/src/commands/ui.rs:351-384`, `forge/crates/core/src/commands/ui.rs:461-472`). Watch callbacks have the same pattern for callback mutations (`forge/crates/core/src/commands/watch.rs:607-637`). Either reject before entering applet code with a reservation/estimate, run applet effects and run-record persistence in one rollbackable unit, or make run-log quota a report/warning path that never blocks the durable run record. Add tests that assert an over-`run_logs_cap` `runtime.run`, UI dispatch, and watch callback leave no new records/UI state behind when they report rejection.

Checks:

- `cargo test -p forge-core --test quota_run_logs_cap --offline`
- `cargo test -p forge-core --test quota_core_conformance --offline`
- `cargo test -p forge-storage --test quota_fixtures --offline`
