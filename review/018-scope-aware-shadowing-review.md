# Review 018: scope-aware shadowing follow-up

Commit reviewed: `33bec5a9` (`forge-pipeline: fix harden static scan - scope-aware shadowing`)

## Findings

1. **[P1] Block/catch-scoped shadows still suppress forbidden globals outside their scope.**  
   The commit fixes sibling function/method shadows, but `ScopeBindingCollector` still flattens every `var`/`let`/`const` declarator and `catch` binding into the current function/module frame (`forge/crates/pipeline/src/scan.rs:386`, `forge/crates/pipeline/src/scan.rs:401`, `forge/crates/pipeline/src/scan.rs:430`). Because `check_forbidden_ref` suppresses any forbidden bare identifier present in any current frame (`forge/crates/pipeline/src/scan.rs:516`), a block-only binding can still hide the real global at an outer use site:
   ```ts
   export function leak() {
     if (true) { let fetch = (x: string) => x; }
     return fetch("https://example.com");
   }
   ```
   The `fetch` call resolves to the raw network global, not the block local, but the collector has already added `fetch` to the function frame. Same shape applies to top-level block `let process` before `process.env`, and to `catch (require) {}` before `require("fs")`. Please add real block/catch frames, or collect only function/module-scoped declarations (`var`, function/class names) into the wider frame while keeping `let`/`const`/catch bindings scoped to their block.

2. **[P2] Bypass corpus can still silently skip malformed manifest rows.**  
   `Case.expect` remains an unchecked `String` (`forge/crates/pipeline/tests/bypass_corpus.rs:41`), both tests skip rows whose value is not exactly `"rejected"` or `"allowed"` (`forge/crates/pipeline/tests/bypass_corpus.rs:68`, `forge/crates/pipeline/tests/bypass_corpus.rs:127`), and the final assertions are lower bounds (`forge/crates/pipeline/tests/bypass_corpus.rs:115`, `forge/crates/pipeline/tests/bypass_corpus.rs:151`). A typo like `"rejectd"` can remove a fixture from both tests while the suite stays green as long as counts remain above the floor. Prefer a deserialized enum plus exact expected counts or a single manifest partition assertion.

## Notes

- The original sibling-function shadowing regression from review 016 appears covered by the new `shadow_cross_scope_*` fixtures.
- No new files appeared in `task-between-claude-and-codex` during this check.

## Verification

- `cargo test --locked -p forge-pipeline` from `forge/` passed.
- `cargo build --locked -p forge-pipeline --target wasm32-unknown-unknown` from `forge/` passed.
- `git show --check 33bec5a9` passed.
