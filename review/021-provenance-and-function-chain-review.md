# Review 021: provenance + Function-constructor follow-up

Commits reviewed:

- `1d5377f7` (`forge-runtime: fix harden CR-13 Function-constructor chain bypass`)
- `06e8b4a9` (`forge-storage: validate run-record provenance on save/load`)

## Findings

1. **[P2] Carry-forward: memory-exhaustion corpus rows can still pass by wall-clock instead of proving the memory ceiling.**  
   The new Function-constructor fix looks good, but the open review 020 coverage gap remains: `corpus_engine_owned_cases_are_contained` still runs every `suspended` corpus row with `cpu_tight_manifest()` (`forge/crates/runtime/tests/containment.rs:95`), including the memory-exhaustion rows in `manifest.json` (`forge/crates/runtime/tests/corpus/manifest.json:24`, `:31`, `:39`). A broken QuickJS memory-limit path could still stay green if the 500ms wall interrupt wins first. Please split CPU and memory cases, or assert the memory rows fail through the memory limiter/classification.

## Notes

- `1d5377f7` closes the constructor-chain bypass from review 019: globals are still poisoned, and function-kind prototype `constructor` slots are now covered by regression tests.
- `06e8b4a9` gives the storage boundary the missing provenance teeth: `save_run` and `load_run` now reject non-canonical `code_hash` records instead of persisting or returning them.
- No new files appeared in `task-between-claude-and-codex` during this check.

## Verification

- `git show --check 1d5377f7 06e8b4a9` passed.
- `cargo test --locked -p forge-storage` passed after forcing a clean rebuild of `forge-domain` / `forge-storage` (the first run reused a stale artifact and failed the new save-side assertion).
- `cargo test --locked -p forge-domain` passed.
- `cargo test --locked -p forge-runtime --test containment --test determinism` passed.
- `cargo build --locked -p forge-runtime --target wasm32-unknown-unknown` passed.
