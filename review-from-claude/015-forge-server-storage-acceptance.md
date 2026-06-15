# Review 015 - Forge server storage acceptance

- **Slice goal:** Replace the legacy server DB-schema acceptance test that read `server/src/main.zig` with a Forge server file-backed storage acceptance smoke.
- **Reviewed:** working diff for `forge/crates/server/src/lib.rs` and reference-host server storage acceptance test replacement.
- **Files changed:** `forge/crates/server/src/lib.rs`, deleted `tools/reference-host/test/server-db-schema-acceptance.test.js`, added `tools/reference-host/test/forge-server-storage-acceptance.test.js`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `cargo fmt --package forge-server`; `node --test --no-warnings tools/reference-host/test/forge-server-storage-acceptance.test.js` -> passed; `cargo test -p forge-server --locked` -> passed; `node --no-warnings tools/check-repo.mjs` -> passed.

## Findings

No blocking findings.

- [P2] The removed test validated legacy SQLite/Postgres schema parity, while Forge server currently delegates persistence to `WorkspaceCore` and Forge storage. Resolution: acceptable for the cutover because the legacy server schema is not a Forge deletion gate once public storage contracts live in Forge tests/contracts.
- [P3] The new Rust test uses `SystemTime::now()` to create a unique temp filename. Resolution: acceptable in a test-only path; no replay/runtime determinism surface is affected.

## Resolution status

- `forge-server` now has a file-backed workspace test that opens storage, sends `workspace.open` through `/bridge`, reopens the same path, and verifies `/health`.
- Reference-host storage acceptance runs the named Forge server Cargo test instead of parsing `server/src/main.zig`.
- `server-db-schema-acceptance.test.js` is removed.

## Follow-ups

- Keep Forge storage schema/projection guarantees covered in `forge-storage` and public-contract fixtures.
- Replace or retire the remaining legacy `server-bridge-contract.test.js` after the Forge server compatibility decision.
