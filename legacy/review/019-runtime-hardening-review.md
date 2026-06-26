# Review 019: runtime hardening follow-up

Commit reviewed: `03ab2c98` (`forge-runtime: harden code_hash/CR-9/eval-poison/replay`)

## Findings

1. **[P1] The eval/Function engine hardening does not work; the new tests fail.**  
   `disable_dynamic_eval` only sets global properties to `undefined` (`forge/crates/runtime/src/engine.rs:655`), but QuickJS still exposes working `eval` / `Function` bindings to evaluated code. The regression tests added in this commit fail: `typeof eval` is still `"function"`, `eval("1 + 1")` completes with `2`, and `new Function("return 1")()` completes with `1` (`forge/crates/runtime/tests/containment.rs:278`). That means CR-13 is still relying on the static scan and the commit's engine-level second line of defense is absent. Please fix the realm-level eval mode rather than only assigning global properties, and add a constructor-chain case too (`(() => {}).constructor("return 1")()`), since hiding `globalThis.Function` alone does not remove Function constructors reachable through existing function objects.

2. **[P2] Older run records deserialize but replay with an all-deny permission snapshot.**  
   `RunRecord.permissions` defaults to `PermissionSnapshot::default()` for old records (`forge/crates/domain/src/run.rs:83`), where `can_run = false` and `max_host_calls = 0` (`forge/crates/domain/src/run.rs:30`). Replay then unconditionally builds policy and limits from that snapshot (`forge/crates/runtime/src/runner.rs:102`), so an older record that loaded successfully but contains any `time`, `random`, storage, db, ui, or log call will now replay as a permission/resource denial instead of using the caller-provided manifest. If backward replay compatibility matters, distinguish "snapshot absent" from an explicit all-deny snapshot and fall back or migrate deliberately.

## Notes

- No new files appeared in `task-between-claude-and-codex` during this check.
- The canonical `sha256:` record path and `RunRecord::new` adoption look directionally good, but the failing runtime suite should block relying on this commit.

## Verification

- `git show --check 03ab2c98` passed.
- `cargo build --locked -p forge-domain -p forge-policy -p forge-runtime --target wasm32-unknown-unknown` from `forge/` passed.
- `cargo test --locked -p forge-runtime -p forge-domain -p forge-policy` from `forge/` failed in `forge-runtime --test containment`:
  - `eval_and_function_globals_are_poisoned`
  - `calling_poisoned_eval_is_a_runtime_error_not_execution`
  - `new_function_constructor_is_a_runtime_error`
