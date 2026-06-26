# Review 177 - DL-22 core quota wiring (f0421b2f)

Findings:

- [P1] Enforce `run_logs_cap` on every run persistence path. DL-22 explicitly includes caps for run logs (`prd-merged/02-data-layer-prd.md:78`), and storage accounting charges both `run_logs` and `runs` to `QuotaCategory::RunLogs` (`forge/crates/storage/src/quota.rs:401-410`). The new command surface lets an Owner set `run_logs_cap`, but `runtime.run` still persists run records with `save_run_tx` inside a transaction that has no quota gate (`forge/crates/core/src/commands/runtime_run.rs:241-252`), while UI/watch callback paths call `self.store.save_run(&run)?` directly (`forge/crates/core/src/commands/ui.rs:398`, `forge/crates/core/src/commands/ui.rs:434`, `forge/crates/core/src/commands/ui.rs:461`, `forge/crates/core/src/commands/watch.rs:633`). That makes the run-log cap report-only: after tightening the cap, later runs can keep appending `runs.record_json` bytes beyond the configured limit. Add a tx-scoped run-log quota check after staging the run/audit rows, or a shared `save_run_with_quota_tx` helper, and cover `runtime.run`, `ui.dispatch_event`, and watch callbacks with over-cap tests.

- [P2] Make `quota.status.approaching` enumerate all approaching budgets, not just one `decide_quota` result. The command docs and spec promise every workspace/applet/category budget already at or above threshold (`forge/crates/core/src/commands/quota.rs:46-52`, `forge/spec/quotas.md:193-198`). However `approaching_warnings` calls `decide_quota` as a proxy for each group (`forge/crates/core/src/commands/quota.rs:121-145`), and `decide_quota` intentionally returns only the first over-cap scope or the single strongest approaching scope (`forge/crates/storage/src/quota.rs:611-687`). So if the workspace budget and retained-chunks cap are both above threshold, the category can win and the workspace warning is omitted; similar masking can happen for applets. Build the status list by directly comparing each current usage bucket against its own limit/threshold, then add a fixture with simultaneous workspace, applet, and category warnings.

Checks:

- `cargo test -p forge-core --test quota_core_conformance --offline`
- `cargo test -p forge-storage --test quota_fixtures --offline`
