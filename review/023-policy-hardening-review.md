# Review 023: policy hardening follow-up

Commit reviewed: `83f6d8a9` (`forge-policy: harden gate scoping, non-ambient seams, glob grants`)

## Findings

1. **[P2] Real `DecisionContext` denials will not replay as recorded denials.**  
   The new context seam is public via `PolicyEngine::with_context` (`forge/crates/policy/src/lib.rs:356`), but replay always rebuilds policy with `AllowAll` (`forge/crates/policy/src/lib.rs:380`, `forge/crates/policy/src/lib.rs:392`). The comment says the snapshot captured non-capability gate outcomes, yet `PermissionSnapshot` only stores capabilities, `can_run`, and `max_host_calls` (`forge/crates/domain/src/run.rs:30`). A call denied only by workspace/run-profile/platform context is recorded through `HostContext::check_or_record_denial` (`forge/crates/runtime/src/host.rs:111`), but replay will allow that same call, skip `record_denial`, and then `RunRecorder::host_call` consumes the recorded `{"denied": ...}` payload as a normal response (`forge/crates/runtime/src/recorder.rs:215`, `forge/crates/runtime/src/recorder.rs:273`). Current `record_run` still uses the `AllowAll` path (`forge/crates/runtime/src/runner.rs:49`), so this is a sharp edge for the newly introduced seam rather than a current M0a path failure. Before wiring a real context, either snapshot the evaluated context decision or make replay detect recorded denial payloads and reconstruct the error before the live-allowed path proceeds.

2. **[P2] Cloning a context-scoped policy silently drops back to `AllowAll`.**  
   `PolicyEngine::clone` copies the capability grants and budgets but replaces any custom `DecisionContext` with `AllowAll` (`forge/crates/policy/src/lib.rs:328`, `forge/crates/policy/src/lib.rs:336`). Once M0b starts passing real workspace/run-profile/platform gates through `with_context`, any clone of the engine becomes a permission bypass for those gates. Consider removing `Clone` from `PolicyEngine`, representing contexts as a cloneable enum/handle, or using a `dyn_clone`-style trait so a cloned engine preserves the same gate behavior. A small test cloning a deny-context policy would pin this down.

## Notes

- This commit does address the review 006 shape: storage bare-star grants are now rejected, time/random flow through policy, and the SC-10 capability subcheck is separated from the missing context gates.
- No new files appeared in `task-between-claude-and-codex` during this check.

## Verification

- `git show --check 83f6d8a9` passed.
- `cargo clean -p forge-policy -p forge-runtime` passed from the `forge/` workspace.
- `cargo test --locked -p forge-policy` passed.
- `cargo test --locked -p forge-runtime --test containment --test determinism` passed.
- `cargo clippy --locked -p forge-policy -- -D warnings` passed.
- `cargo build --locked -p forge-policy --target wasm32-unknown-unknown` passed.
- `cargo build --locked -p forge-runtime --target wasm32-unknown-unknown` passed.
