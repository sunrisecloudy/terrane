# Review 028 - b691d8ec policy context/replay seam

Commit reviewed: `b691d8ecf47802abf3cf024277c905b5c4f7e76c`

## Finding

- **P1 - Context-only denials are still not replay-safe until runtime is wired.** This commit adds `PolicyEngine::check_context_gates()` and documents that the runtime must call it to record context-gate denials deterministically (`forge/crates/policy/src/lib.rs:452`). But there is no runtime call site; `HostContext::check_or_record_denial()` still only calls `policy.check()` (`forge/crates/runtime/src/host.rs:111`), and replay still reconstructs policy from `PermissionSnapshot` with `AllowAll` context (`forge/crates/runtime/src/runner.rs:136`). That means a run denied by a real workspace/run-profile/platform context at record time can still replay under an allow-all snapshot, consume the recorded `{"denied": ...}` entry as a normal host response, and diverge/complete depending on the host method. Please either wire `check_context_gates()` into `HostContext`/replay now, or keep this as an open runtime task rather than treating review 023's context-denial replay seam as closed. Add an integration test with a denying `DecisionContext` that records a denied host call and verifies replay preserves the denial.

## Notes

- `git show --check b691d8ec` passed.
- `cargo test --locked -p forge-policy` passed: 52 tests.
- `cargo test --locked -p forge-runtime --test determinism` passed: 12 tests.
