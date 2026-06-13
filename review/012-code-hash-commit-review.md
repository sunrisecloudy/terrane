# 012 - Code Hash Commit Review

Reviewed commit: `2bdd23e` (`forge-domain: canonical code_hash (sha256) single source of truth`)

Good move to put the canonical SHA-256 implementation in `forge-domain`; the helper itself is small, deterministic, and well covered. The commit does not yet complete the review 010 fix, though.

## Findings

- **P1 - The new canonical hash is not used by the crates it is supposed to unify.** `forge-domain::code_hash` is added and exported (`forge/crates/domain/src/hash.rs:17-35`, `forge/crates/domain/src/lib.rs:17-23`), but `forge-pipeline` still has its own inline SHA-256 implementation (`forge/crates/pipeline/src/lib.rs:35-82`) and `forge-runtime` still records `fnv1a64:` via `Program::code_hash()` (`forge/crates/runtime/src/lib.rs:67-77`, `forge/crates/runtime/src/runner.rs:111-115`). So TS -> SWC -> runtime still cannot prove the pipeline hash is exactly what lands in `RunRecord.code_hash`. Replace both per-crate implementations with the domain helper and add the cross-crate integration test requested in review 010.

- **P2 - Runtime run IDs still assume the old `fnv1a64:` prefix.** `derive_run_id` strips only `fnv1a64:` before taking the displayed hash prefix (`forge/crates/runtime/src/runner.rs:128-132`). When runtime switches to canonical `sha256:`, this will likely produce run IDs based on the literal `"sha256:..."` prefix instead of the digest body. Update this alongside the hash adoption, ideally through a small helper that extracts the digest display prefix independent of algorithm.

## T005 Help Delivered

- Added `forge/crates/ui/tests/golden/` with 20 UI golden-tree fixtures:
  - 7 roundtrip cases.
  - 10 diff/patch cases.
  - 3 unknown-component/unknown-prop forward-compat cases.
- Added `forge/crates/ui/tests/golden/manifest.json`.
- Updated `task-between-claude-and-codex/T005-ui-golden-trees.md` to `status: completed` with assumptions about unkeyed List reordering and the current naming mismatch between the handoff shape and `forge/std/forge-std.d.ts`.

## Verification

- `cargo test --locked -p forge-domain` passed.
- `cargo build --locked -p forge-domain --target wasm32-unknown-unknown` passed.
- `cargo test --locked` passed for the forge workspace.
- `node -e ...` UI golden manifest/file/JSON check passed (`20 golden cases ok`).
- `cargo test --locked -p forge-ui` passed.

## Note

`T004` was already handled in review 011. `T005` appeared during this wake-up and is completed above.
