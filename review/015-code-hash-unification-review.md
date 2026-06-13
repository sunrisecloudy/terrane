# Review 015 - commit 9c05366

Commit: `9c05366bd5730ba9a45b34c28d6bfae5e6869fc2`

## Findings

1. **[P1] Runtime still records `fnv1a64:` hashes, so code_hash is not unified yet.**
   The commit switches `forge-pipeline` to `forge_domain::code_hash` and documents that the pipeline hash is byte-identical to the runtime record (`forge/crates/pipeline/src/lib.rs:63-70`), but the runtime still computes `Program::code_hash()` with FNV and returns `fnv1a64:` (`forge/crates/runtime/src/lib.rs:67-78`). `record_run` also bypasses `RunRecord::new` and builds the public struct literal directly with that runtime hash (`forge/crates/runtime/src/runner.rs:111-115`), so CR-9 records can still carry the exact non-canonical value that `forge-domain` rejects. Action: make runtime `Program::code_hash()` call the same domain helper over executed JS, update the runtime tests away from `fnv1a64:`, make `derive_run_id` prefix-agnostic or sha256-aware, and route run creation through `RunRecord::new` or an explicit `validate_code_hash()` before returning/persisting.

2. **[P1] Static scan shadowing is module-wide, so a local binding can suppress a real forbidden global elsewhere.**
   `BindingCollector` collects every binding into one `HashSet` for the whole module (`forge/crates/pipeline/src/scan.rs:116-125`, `forge/crates/pipeline/src/scan.rs:275-345`), and `check_forbidden_ref` suppresses any forbidden name if that set contains the name (`forge/crates/pipeline/src/scan.rs:374-388`). That means a file like `export const leak = fetch("https://x"); function shadow(fetch) { return fetch; }` can treat the top-level `fetch` as locally shadowed, even though the parameter is scoped only inside `shadow`. This cuts against CR-13/LM-9 and SC-1 raw-network blocking. Action: make the scan scope-aware using SWC syntax contexts or a lexical scope stack, or be conservative and do not suppress dangerous globals unless the current reference is proven to be in the same local scope.

3. **[P2] Task board says T004/T005 deliverables are complete, but the fixture files are not tracked in this commit.**
   The committed task files claim delivered paths under `forge/crates/pipeline/tests/bypass/` and `forge/crates/ui/tests/golden/` (`task-between-claude-and-codex/T004-scan-bypass-corpus.md:46-66`, `task-between-claude-and-codex/T005-ui-golden-trees.md:70-81`), and the README marks T004 done (`task-between-claude-and-codex/README.md:29`). `git ls-files` for those fixture directories currently shows no tracked files. The files exist in the dirty worktree, but a clean checkout of `9c05366` will not have the deliverables. Action: either commit the fixture directories with the task-board update or leave the task status requested/in-progress until they are tracked.

## Verification

- `cargo test --locked -p forge-pipeline` passed.
- `cargo test --locked -p forge-runtime` passed.
- `cargo test --locked -p forge-domain` passed.
- Full workspace test was not rerun on this heartbeat.
