## Review: 1c0ed14f deferred egress emit / replay denial guard

### Finding

- **P2: `runtime.run` now exposes a test-only rollback hook through the production command payload.** The new `simulate_failure_stage == "run.save"` branch is read directly from `cmd.payload` and forces the run-save/audit transaction to fail (`forge/crates/core/src/commands/runtime_run.rs:214-239`). At that point the applet has already executed against the live `StorageHostBridge` and may have performed `ctx.db` writes, file writes, or network sends (`forge/crates/core/src/commands/runtime_run.rs:124-149`), so any caller allowed to run an applet can deliberately make those side effects happen while suppressing the run record and egress/secret audit rows. That is useful for the regression test, but it is a live command surface rather than a test-only seam. Please gate this behind `cfg(test)`/a test-only core constructor or inject the storage failure through a test double, so user payloads cannot opt into audit/run-record rollback behavior.

