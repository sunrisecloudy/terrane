# Commit Review: 3c2cde6b

Reviewed commit: `3c2cde6b forge-cli: M0a spine harness + notes-lite demo + e2e proof`

## Findings

1. **P2 - The scenario corpus bypasses the command/event facade it is meant to prove.** `tests/scenarios.rs` explicitly avoids `forge-core` because `runtime.run` cannot accept fixture seeds yet (`forge/crates/cli/tests/scenarios.rs:8`) and then composes `forge_pipeline::compile`, `forge_runtime::record_run`, and `StorageHostBridge` directly (`forge/crates/cli/tests/scenarios.rs:147`). That is useful runtime coverage, but it does not exercise `WorkspaceCore::handle`, `applet.install`, `runtime.run`, `runtime.replay`, command authorization, run persistence, or the CLI shell contract that PS-5 requires (`prd-merged/06-platform-shells-prd.md:17`, `prd-merged/09-roadmap-quality-gates-prd.md:18`). Please either plumb deterministic `random_seed`/`time_start` through the command payload and run these fixtures through `WorkspaceCore`, or normalize the fixture expectations to the core defaults. Keep direct runtime coverage separately, but do not count it as the CLI harness acceptance proof.

2. **P2 - Denial-code equivalence hides fixture/spec drift.** `denied_capability/expect.json` asks for `PermissionDenied` (`forge/fixtures/e2e/denied_capability/expect.json:5`), but the test accepts either `PermissionDenied` or `CapabilityRequired` (`forge/crates/cli/tests/scenarios.rs:245`, `:272`). The error catalog distinguishes these cases: `PermissionDenied` is RBAC, while `CapabilityRequired` means the applet lacks a host capability (`forge/spec/errors.md:8`). If the current policy behavior is correct, update the fixture to `CapabilityRequired` and assert exact equality; if `PermissionDenied` is intended, change the manifest to declare some db scope but not `audit_log`. The conformance fixtures should be normative, not fuzzily matched.

## Verification

- `git show --check 3c2cde6b`
- `cargo test --locked -p forge-cli`
- `cargo clippy --locked -p forge-cli --all-targets -- -D warnings`
- `cargo run --locked -p forge-cli -- demo`
