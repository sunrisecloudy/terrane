# Review 009 — package release Forge server

- **Slice goal:** Phase 2.6 server packaging cutover: make `--build-server` package the new Rust `forge-server` binary instead of compiling legacy `server/src/main.zig`.
- **Reviewed:** working diff in release packaging, release packaging tests, and checker snippets.
- **Files changed:** `tools/package-release.mjs`, `tools/reference-host/test/release-packaging.test.js`, `tools/check-repo.mjs`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --test --no-warnings tools/reference-host/test/release-packaging.test.js` -> Forge server subtest passed, but the full file failed in sandbox during macOS `hdiutil create`; rerun unsandboxed -> passed; `node --no-warnings tools/check-repo.mjs` -> passed.

## Findings

No blocking findings.

- [P2] The full release-packaging test still hits sandbox friction in the macOS DMG subtest. Resolution: reran the exact test unsandboxed; all six tests completed with four pass and two platform skips.
- [P3] The release artifact path remains `server/<target>/terrane-server` for compatibility, even though it is now built from the `forge-server` Cargo binary. Resolution: acceptable for this cutover slice; downstream artifact names can be cleaned up after CI/release consumers are repointed.

## Resolution status

- `buildServerArtifacts` now runs `cargo build -p forge-server --release --locked` from `forge/`.
- The built `forge-server` binary is copied to the existing release path as `terrane-server` / `terrane-server.exe`.
- Release manifest server artifacts now use `kind: "forge-server-executable"`.
- `tools/check-repo.mjs` checks for the Forge server packaging surface.

## Follow-ups

- Repoint CI/release workflow job labels away from legacy `server-release-artifacts` once the workflow itself is cut over.
- Later remove the compatibility `terrane-server` output name if no downstream release consumer still expects it.
