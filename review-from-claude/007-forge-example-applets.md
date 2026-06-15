# Review 007 — Forge example applets

- **Slice goal:** Phase 1.5 first gap-fill slice: provide Forge-native replacement example applets before repointing packaging/runtime consumers away from legacy `webapps/examples/`.
- **Reviewed:** working diff adding four missing `forge/examples/*` applets, a data-driven `forge-cli` example harness, and example coverage docs.
- **Files changed:** `forge/examples/api-dashboard`, `forge/examples/core-replay-lab`, `forge/examples/file-transformer`, `forge/examples/task-workbench`, `forge/docs/example-applets.md`, `forge/crates/cli/Cargo.toml`, `forge/crates/cli/tests/forge_examples.rs`, `tools/export-public-contract.mjs`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `cd forge && cargo fmt --package forge-cli` -> passed; `cd forge && cargo test -p forge-cli --test forge_examples --locked` -> passed; `cd forge && cargo test -p forge-cli --locked` -> passed; `cd forge && cargo clippy -p forge-cli --all-targets --locked -- -D warnings` -> passed; `cd forge && cargo run -p forge-cli -- demo` -> passed with `REPLAY IDENTICAL: true`; `node --test --no-warnings tools/reference-host/test/public-contract.test.js` -> passed; `node --no-warnings tools/export-public-contract.mjs --out /private/tmp/terrane-public-contract.json` -> passed; `node --no-warnings tools/verify-public-contract.mjs --contract /private/tmp/terrane-public-contract.json --root /Users/vehasuwat/Project/terrane` -> passed, `filesChecked=457`.

## Findings

No blocking findings.

- [P3] `@forge/std` currently omits type declarations for `ctx.net` and `ctx.files` even though the runtime exposes them. Resolution: the examples use `(ctx as any).net/files` for now, and the executable harness proves the runtime surface works. A typed std update should be handled separately.
- [P3] This slice provides Forge replacement examples but does not repoint release packaging, runtime-web, native resource locators, or reference-host tests away from legacy `webapps/examples/`. Resolution: record as the next Phase 1.5/Phase 2 packaging-runtime cutover work, not mixed into the example-source commit.

## Resolution status

- Added Forge equivalents for the legacy coverage areas: task workflow, file transform, API dashboard, core replay lab, alongside existing notes-lite.
- Added `forge-cli` test coverage that installs every example from disk, injects mock network/filesystem seams, runs through `WorkspaceCore::handle`, checks data/UI output, and verifies byte-identical replay.
- Added `forge/docs/example-applets.md` and included it in the public-contract export.

## Follow-ups

- Add typed `ctx.net` and `ctx.files` declarations to `forge/std/forge-std.d.ts`.
- Repoint packaging/runtime/native/reference-host consumers from `webapps/examples` to the Forge example set in later focused commits.
