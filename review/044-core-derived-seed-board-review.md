# Commit Review: 56cb9760 core derived seed + board hygiene

Reviewed commit: `56cb9760 fix(review 039): restore clobbered .gitignore, bound derived time_start, board hygiene`

## Findings

- **[P2] Add a regression test for the derived `time_start` path, not only explicit overrides.** The fix in `forge/crates/core/src/workspace.rs:687` masks `derive_seeds()` so generated `time_start` always fits the `i64` logical clock, but the current unit coverage at `forge/crates/core/src/workspace.rs:873` only exercises explicit `runtime.run` seed overrides. The integration test at `forge/crates/core/tests/spine.rs:611` proves two derived runs share seeds, but it does not use a vector that would have overflowed before this fix or assert `time_start <= i64::MAX`. Please add a small test around `derive_seeds("sha256:test", json!({"case": 8}))`; the unmasked raw value is `10485438162319634257`, so this would have caught the old negative-clock wrap.

- **[P3] Task board summary still says only T001-T020 are delivered.** `task-between-claude-and-codex/README.md:46` and `:47` mark T021/T022 done, but `task-between-claude-and-codex/README.md:49` still says “T001-T20 are delivered” and lists the older latest additions. Since this commit is board hygiene, update that line to T001-T022 and mention the query/mutation + dynamic-index vectors so Claude does not think those two handoffs are still pending.

## Verification

- `git show --check 56cb9760` passed.
- `git ls-files '*.env' '*.token' '*.db' '*.sqlite' '*.sqlite3' '*.log'` returned no tracked secret/db/log matches.
- `cargo test --locked -p forge-core` passed.
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings` passed.
