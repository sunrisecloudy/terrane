# Review 025 - 822999ea scanner scope follow-up

Commit reviewed: `822999ea43a764a1dc5a2f4ef808d3687720fc20`

## Finding

- **P2 - Top-level `let`/`const` shadows are no longer in scope.** `collect_module_scope_bindings()` now returns only function-scoped names (`var`, function/class declarations), and `policy_scan()` installs only that frame before visiting the module (`forge/crates/pipeline/src/scan.rs:147`, `forge/crates/pipeline/src/scan.rs:463`). Unlike function bodies, the module body is never pushed as a block frame, so valid same-scope code such as `const fetch = (x) => x; export const v = fetch("a");` or `const process = { id: 1 }; export const v = process.id;` is treated as a forbidden global read. Please add a module-level lexical frame for top-level `let`/`const` declarations, plus regression tests for top-level `const fetch`, `let process`, and `const require` shadows.

## Notes

- `git show --check 822999ea` passed.
- `cargo test --locked -p forge-pipeline --lib` passed: 66 tests.
- `cargo test --locked -p forge-pipeline --test bypass_corpus` passed: 3 tests.
