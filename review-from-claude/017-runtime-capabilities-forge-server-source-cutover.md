# Independent Codex review: runtime capabilities Forge server source cutover

- **Slice goal:** Remove the last active reference-host capability contract read of `server/src/main.zig` while preserving the native runtime capability assertions and documenting the Forge server replacement boundary.
- **Review mode:** Independent Codex/self-review. Claude Code review is intentionally not requested because the user instructed Codex to work independently from Claude Code for this goal.
- **Files changed:** `tools/reference-host/test/runtime-capabilities-contract.test.js`, `docs/08_TEST_PLAN.md`.
- **Remote-owned reason:** `tools/reference-host/test/*` and docs are approved migration files; this edit is required because the old test kept `server/src/main.zig` as a live source dependency after the Forge server bridge-contract replacement.
- **Commands run:**
  - `node --test --no-warnings tools/reference-host/test/runtime-capabilities-contract.test.js` -> passed.
  - `rg -n "server/src/main\\.zig|zig-core-build|zig-crdt-build|server-zig-build|server-bridge-contract|server-db-schema-acceptance" tools/reference-host/test tools/check-repo.mjs docs/08_TEST_PLAN.md docs/13_EXAMPLE_APP_COVERAGE.md` -> no retired test/source-read hits; only Forge bridge-contract docs remained in output.
  - `node --no-warnings tools/check-repo.mjs` -> passed.

## Findings

- No blocker found. The old source-level server capability assertion was v0.4-specific and forced the test to read `server/src/main.zig`.
- The replacement assertion checks the Rust Forge server source for the active replacement HTTP surface: `/health`, `/bridge`, `/events/drain`, `CoreCommand` parsing, and `WorkspaceCore` construction.
- Native source assertions still intentionally check `core.step`/Zig-backed host implementations until the host cutover slices land.

## Resolution

- Split the old "native and server capability implementations" assertion into a native-only runtime capability assertion plus a Forge server CoreCommand HTTP replacement assertion.
- Updated the test plan to say this contract no longer reads the retired Zig server source.

## Follow-ups

- The checked-in `tests/fixtures/capabilities/server.json` fixture still describes the legacy `zig-server` runtime capability envelope. Keep it until the broader legacy fixture/schema deletion gate is satisfied or until a real Forge server capability envelope is designed.
- Native host tests and docs still reference `core.step` and Zig libraries until the per-host Forge FFI ports are complete.
