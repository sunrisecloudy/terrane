# Review 017 - commit b15e3d3d

Commit: `b15e3d3d96e15cdd9906f95151ddd6e79ac34430`

## Findings

1. **[P1] The module-wide shadowing hole from review 016 is still present.**
   This commit adds good coverage for value-position captures, assignment aliases, destructuring from global containers, and template computed keys, but `BindingCollector` is still a module-wide `HashSet` and `check_forbidden_ref` still suppresses any forbidden bare identifier if the name is bound anywhere in the module (`forge/crates/pipeline/src/scan.rs:438-454`). A parameter/local in one function can still hide a real top-level forbidden global in another scope, e.g. `export const leak = fetch("https://x"); function shadow(fetch) { return fetch; }`. Because the commit message explicitly says the static scan is the only code-eval defense when QuickJS binds `intrinsic::All`, this should be fixed before calling the hardening complete. Action: add this cross-scope case to `tests/bypass/`, then either implement scope-aware binding checks or remove bare-identifier shadow suppression for dangerous globals until it can be proven local.

2. **[P2] Manifest rows can still be silently skipped.**
   `bypass_corpus.rs` still stores `expect` as a `String`, skips rows whose value is neither `"rejected"` nor `"allowed"`, and only checks lower bounds (`>= 30`, `>= 6`) (`forge/crates/pipeline/tests/bypass_corpus.rs:41-48`, `forge/crates/pipeline/tests/bypass_corpus.rs:68-70`, `forge/crates/pipeline/tests/bypass_corpus.rs:115-119`, `forge/crates/pipeline/tests/bypass_corpus.rs:127-129`, `forge/crates/pipeline/tests/bypass_corpus.rs:151-154`). A typo or duplicated category can keep passing as long as the minimum count remains satisfied. Action: deserialize `expect` into an enum with `#[serde(rename_all = "lowercase")]` and assert exact per-category counts derived from the manifest, or at minimum reject unknown `expect` values before the loops.

## Verification

- `cargo test --locked -p forge-pipeline` passed.
- `cargo build --locked -p forge-pipeline --target wasm32-unknown-unknown` passed.
- `git show --check b15e3d3d` passed.
