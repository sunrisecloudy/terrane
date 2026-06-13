# 009 - Runtime Commit Review

Reviewed commit: `fff81a5` (`forge-runtime: QuickJS sandbox + capability ctx + deterministic record/replay`)

Hey Claude, this is a strong spine slice: native QuickJS is gated out of wasm, runtime tests are green, replay avoids live bridge calls, and the host-call surface is much closer to the merged PRD. A few issues still look important before we treat this as the CR-8/CR-9/CR-13 baseline.

## Findings

- **P1 - Run records do not yet satisfy CR-9 permission/audit replay requirements.** `RunRecord` currently persists `code_hash`, input, seed/time, calls, logs, and outcome, but not the permission snapshot, resource usage, or resulting writes required by `prd-merged/01-core-runtime-prd.md:52-53`. Replay also accepts a fresh `manifest`/`actor` and rebuilds policy from current state (`forge/crates/runtime/src/runner.rs:58-84`, `forge/crates/runtime/src/host.rs:35-45`). Policy denials happen before `recorder.host_call` for storage/db/ui (`forge/crates/runtime/src/host.rs:76-80`, `88-92`, `149-153`, `193-195`), so denied host API attempts are not recorded as host responses. This means replay/debug/audit can drift if grants or roles change after the original run. Store the evaluated permission snapshot in `RunRecord`, record denied host-call attempts as deterministic error responses, and add a replay test where current permissions differ from the recorded snapshot.

- **P1 - `eval`/`Function` are still available at engine level despite CR-13/SC-1.** The merged PRD says `eval`, `Function`, dynamic import, and bridge prototype pollution are disabled at engine level and rejected by static scan (`prd-merged/01-core-runtime-prd.md:60`, `prd-merged/07-security-prd.md:21`, `prd-merged/04-llm-system-prd.md:37`). The runtime intentionally creates `intrinsic::All` and keeps `eval`/`Function` available (`forge/crates/runtime/src/engine.rs:201-209`), while the hostile corpus still says those cases are disabled at engine level (`forge/crates/runtime/tests/corpus/manifest.json:60-71`). Either remove/poison these globals in the QuickJS realm and add regression tests, or update the PRD/corpus if the contract is changing.

- **P2 - Replay accepts prefix traces unless every caller remembers to compare fingerprints.** `RunRecorder::consume` detects method/arg mismatch and extra live calls past the end (`forge/crates/runtime/src/recorder.rs:220-248`), but there is no end-of-run check that all recorded calls were consumed before `finish_run` returns `Ok` (`forge/crates/runtime/src/runner.rs:100-121`). A tampered/stale recording with extra trailing calls can produce a shorter replay record; that only becomes visible if the caller checks `replays_identically`. Make replay itself fail on unconsumed recorded calls, and add a test that appends one extra recorded call to an otherwise valid trace.

- **P2 - `ctx.log` bypasses host-call budget and capability checks.** `HostContext::log` explicitly skips policy and `max_host_calls` accounting, relying only on `log_bytes` (`forge/crates/runtime/src/host.rs:206-224`). Because empty or tiny lines create recorded calls and stored log entries with near-zero byte cost, applets can flood the recorder/log vector without tripping the host-call budget. Either count log through `PolicyEngine` like every other `ctx.*` syscall, or introduce an explicit log-call count limit plus a test for empty-string log floods.

## Verification

- `cargo test --locked -p forge-runtime` passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-runtime` passed.
- `cargo test --locked` passed for the forge workspace.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` passed.
- `cargo check --locked --target wasm32-unknown-unknown` at workspace scope still fails in `sqlite-wasm-rs` C compilation. Not necessarily caused by this commit, but worth documenting because the M0a PRD gate is full spine/WASM, not just runtime-only.
