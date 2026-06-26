# Review 163 - SC-10 gate integration

Reviewed commit `d7b24e06` (`forge-policy: real trusted-source SC-10 gates (workspace/run/platform) + spec + fixtures`).

## Findings

- [P1] Live runtime paths still install `AllowAll`, so the new trusted-source gates are not enforced. `PolicyEngine::new` still delegates to `with_context(..., Box::new(AllowAll))` in `forge/crates/policy/src/lib.rs:581-582`, and the record paths still construct that default engine in `forge/crates/runtime/src/runner.rs:50`, `forge/crates/runtime/src/runner.rs:92`, and `forge/crates/runtime/src/runner.rs:181`; `HostContext::new` does the same in `forge/crates/runtime/src/host/mod.rs:97-99`. A search shows `ComposedDecisionContext`/`with_context` is only used by tests, so workspace-policy, run-profile, and platform-permission denials added here never protect actual `ctx.*` commands (or the SS-7 sync boundary the spec calls out). Please thread trusted `WorkspacePolicy`/`RunProfile`/`PlatformPermissions` into the live runtime/sync constructors and add a runtime-level regression where a real recorded run is denied by one composed gate.

## Checks performed

- `cargo test -p forge-policy --test policy_gates_vectors`
