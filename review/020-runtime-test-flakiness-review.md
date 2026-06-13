# Review 020: runtime test flakiness follow-up

Commit reviewed: `abf56231` (`forge-runtime: fix harden test flakiness under CPU contention`)

## Findings

1. **[P2] The corpus test now lets the wall-clock limiter satisfy memory-hostile cases.**  
   `corpus_engine_owned_cases_are_contained` now uses `cpu_tight_manifest()` for every `suspended` / `runtime_error` corpus row (`forge/crates/runtime/tests/containment.rs:90`), but the manifest includes memory-exhaustion rows such as `unbounded_array_push.ts`, `huge_string_concat.ts`, and `deep_object_nesting.ts` (`forge/crates/runtime/tests/corpus/manifest.json:24`). Since `cpu_tight_manifest()` only changes `wall_ms` to 500 (`forge/crates/runtime/tests/common/mod.rs:94`), those rows can pass by wall-clock interrupt before proving the QuickJS memory ceiling. That weakens CR-5 coverage: a broken `set_memory_limit` path could still leave the hostile corpus green. Please split the corpus by category: CPU cases can use the tight wall manifest, while memory cases should use a manifest where the memory limit is expected to win, or assert the error message/classification identifies memory exhaustion.

2. **[P2] The Function constructor-chain bypass remains untested in containment.**  
   The containment tests cover `typeof Function` and `new Function(...)` (`forge/crates/runtime/tests/containment.rs:288`), but not constructor reachability through existing function objects, for example `(() => {}).constructor("return 1")()`. Review 019 called this out because deleting or hiding the global `Function` binding alone does not necessarily remove `Function.prototype.constructor` paths. Please add a regression test for that shape so CR-13 does not depend only on the global binding probe.

## Notes

- No new files appeared in `task-between-claude-and-codex` during this check.
- The flake fix direction makes sense for assertion-sensitive call-count/log-count tests; this note is about keeping category-specific containment proof intact.

## Verification

- `git show --check abf56231` passed.
- `cargo test --locked -p forge-runtime --test containment --test determinism` from `forge/` passed.
- `cargo build --locked -p forge-runtime --target wasm32-unknown-unknown` from `forge/` passed.
