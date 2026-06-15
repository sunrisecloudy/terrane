# Review 006 — public contract Forge export

- **Slice goal:** Phase 1.4 gap fill: repoint the public-contract export away from legacy `tests/` + `schemas/` inputs and onto Forge v1 docs/specs/contracts/fixtures.
- **Reviewed:** working diff updating the public-contract exporter, verifier default schema path, focused reference-host tests, release-packaging assertion, checker hook, and docs.
- **Files changed:** `tools/export-public-contract.mjs`, `tools/verify-public-contract.mjs`, `tools/check-repo.mjs`, `tools/README.md`, `tools/reference-host/test/public-contract.test.js`, `tools/reference-host/test/release-packaging.test.js`, `docs/35_PUBLIC_CONTRACT_EXPORT.md`, `forge/contracts/public-contract.schema.json`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --check tools/export-public-contract.mjs` -> passed; `node --check tools/verify-public-contract.mjs` -> passed; `node --no-warnings tools/export-public-contract.mjs --out /private/tmp/terrane-public-contract.json` -> passed; `node --no-warnings tools/verify-public-contract.mjs --contract /private/tmp/terrane-public-contract.json --root /Users/vehasuwat/Project/terrane` -> passed, `filesChecked=448`; `node --test --no-warnings tools/reference-host/test/public-contract.test.js` -> passed; `node --test --no-warnings tools/reference-host/test/release-packaging.test.js` -> failed in sandbox during `hdiutil create`, then passed when rerun unsandboxed; `node --no-warnings tools/check-repo.mjs` -> initially failed with `Maximum call stack size exceeded`, then passed after excluding Rust `target/` from the repo walk.

## Findings

No blocking findings after fixes.

- [P2] The first export attempt referenced non-existent PRD paths. Resolution: corrected the public doc list to actual `prd-merged/07-security-prd.md` and `prd-merged/09-roadmap-quality-gates-prd.md`.
- [P2] `tools/check-repo.mjs` recursively walked `forge/target`, causing stack overflow in broad repo checks. Resolution: added `target` to the existing generated-output skip list.
- [P3] The macOS release-packaging test failed in sandbox while creating a DMG with `hdiutil`. Resolution: reran the exact test unsandboxed; it passed, so this is recorded as environment friction rather than a slice regression.

## Resolution status

- Public contract now reports `platformBaseline: "forge-v1-m0b"` and hashes Forge inputs: `docs=23`, `contracts=5`, `fixtures=409`, `tools=11`.
- The exported file list has zero `tests/` or `schemas/` paths.
- `tools/verify-public-contract.mjs` defaults to `forge/contracts/public-contract.schema.json`.
- `tools/check-repo.mjs` validates the contract against the Forge schema and no longer stack-overflows on Rust build output.

## Follow-ups

- `tools/package-release.mjs` still packages legacy `webapps/examples` and Zig/server artifacts. That belongs to Phase 1.5 / Phase 2.6 and is intentionally not mixed into this slice.
- The legacy `schemas/public-contract.schema.json` untracked draft remains in the working tree but is no longer used by the Forge-backed exporter; remove or ignore it during the later legacy `schemas/` deletion gate.
