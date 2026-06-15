# Review 014 - check-repo Forge server static

- **Slice goal:** Repoint `tools/check-repo.mjs` `server.static` from legacy `server/src/main.zig` assertions to the Forge server replacement surface.
- **Reviewed:** working diff for `tools/check-repo.mjs`.
- **Files changed:** `tools/check-repo.mjs`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --no-warnings tools/check-repo.mjs` -> passed; `node --test --no-warnings tools/reference-host/test/forge-server-build.test.js` -> passed.

## Findings

No blocking findings.

- [P2] `native.static` still validates Zig-backed host bridges. Resolution: intentional; those checks remain live until the host-specific Forge FFI ports land.
- [P2] The Forge server static check covers the current minimal `/health`, `/bridge`, and `/events/drain` spine, not full v0.4 control-plane parity. Resolution: acceptable for this cutover step; deeper reference-host server bridge/control compatibility remains a separate Phase 2.10 follow-up.

## Resolution status

- `checkServerStatic()` no longer reads `server/src/main.zig`.
- `server.static` now checks Forge server crate source, CLI args, Cargo dependencies, and the Forge server build smoke.
- `node tools/check-repo.mjs` reports `server.static forge-server=health,bridge,events-drain,core-command,json-http,std-listener,cli-build-test`.

## Follow-ups

- Replace or retire `server-bridge-contract.test.js` and `server-db-schema-acceptance.test.js` once the Forge server compatibility decision is finalized.
- Keep `server/` undeleted until live bridge/control references are removed and zero-grep proves no consumers remain.
