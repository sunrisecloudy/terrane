# Review 016 - commits 40a25ea4 and b55dfcf3

Commits:
- `40a25ea4f6376ca16c6713b9df14183509d04aa0` - collab tasks T006-T017
- `b55dfcf38c8da184f74f5e8eee6c10b55ec9e9c2` - pipeline bypass corpus wiring

## Findings

1. **[P1] Static-scan shadowing is still module-wide, so forbidden globals can still slip past the scanner.**
   The new corpus is useful, but it does not cover the scope bug from review 015. `BindingCollector` still gathers all bindings from the whole module into one `HashSet` (`forge/crates/pipeline/src/scan.rs:275-345`), then `check_forbidden_ref` suppresses a forbidden bare identifier whenever that name exists anywhere in the module (`forge/crates/pipeline/src/scan.rs:374-390`). A parameter or local in one function can therefore suppress a top-level real global elsewhere, e.g. `export const leak = fetch("https://x"); function shadow(fetch) { return fetch; }`. That violates CR-13/LM-9 and SC-1 because raw network can avoid the static layer. Action: either implement lexical/syntax-context-aware shadowing, or be conservative and only suppress dangerous globals when the current reference is proven to be in the same binding scope. Add this exact cross-scope regression to `tests/bypass/`.

2. **[P2] The data-driven corpus can silently skip malformed manifest rows.**
   `bypass_corpus.rs` has two independent loops that continue unless `expect == "rejected"` or `expect == "allowed"` (`forge/crates/pipeline/tests/bypass_corpus.rs:64-68`, `forge/crates/pipeline/tests/bypass_corpus.rs:123-127`) and then only assert lower bounds (`>= 23`, `>= 4`) (`forge/crates/pipeline/tests/bypass_corpus.rs:111-115`, `forge/crates/pipeline/tests/bypass_corpus.rs:147-150`). A typo like `"rejectd"` or a dropped row can be skipped while the lower bound still passes. Action: parse `expect` as an enum with serde, reject unknown values, and assert the checked counts equal the manifest counts for each category.

## Verification

- `cargo test --locked -p forge-pipeline` passed.
- `cargo build --locked -p forge-pipeline --target wasm32-unknown-unknown` passed.
- `git show --check 40a25ea4 b55dfcf3` passed.
