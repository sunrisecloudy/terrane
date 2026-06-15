# Review 013 - reference-host Forge build tests

- **Slice goal:** Replace the standalone reference-host Zig build smoke tests with Forge build/test smokes for the replacement surfaces.
- **Reviewed:** working diff for `tools/reference-host/test/*build.test.js` replacements and `docs/08_TEST_PLAN.md`.
- **Files changed:** deleted `zig-core-build.test.js`, `zig-crdt-build.test.js`, and `server-zig-build.test.js`; added `forge-ffi-build.test.js`, `forge-sync-build.test.js`, and `forge-server-build.test.js`; updated the test plan.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --test --no-warnings tools/reference-host/test/forge-ffi-build.test.js tools/reference-host/test/forge-sync-build.test.js tools/reference-host/test/forge-server-build.test.js` -> passed; `node --no-warnings tools/check-repo.mjs` -> passed.

## Findings

No blocking findings.

- [P2] The deeper reference-host server bridge and native host tests still assert legacy Zig behavior. Resolution: intentionally deferred; those tests are tied to active v0.4 bridge/native surfaces that need separate Forge compatibility or host-porting slices.
- [P3] The new Forge build tests invoke Cargo and therefore write under `forge/target/`. Resolution: acceptable; `forge/target/` is ignored and no generated output is staged.

## Resolution status

- Forge FFI build smoke runs the crate tests, builds release static/shared outputs, checks `forge_ffi.h`, and verifies exported symbols on Unix hosts.
- Forge sync build smoke runs `forge-sync` tests and the FFI sync export/import ABI test that documents CRDT flowing through `forge_core_handle_command`.
- Forge server build smoke runs `forge-server` tests, builds the release executable, and checks the binary exists.

## Follow-ups

- Replace `server-bridge-contract.test.js` with a Forge `/bridge` contract once legacy v0.4 bridge compatibility is either implemented or explicitly retired.
- Repoint native-host build/source tests as each host moves from `libzig_core` to `forge-ffi`.
