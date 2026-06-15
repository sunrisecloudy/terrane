# Review 016 - Forge server bridge contract

- **Slice goal:** Replace the legacy Zig server bridge contract test with a live Forge server HTTP bridge contract.
- **Reviewed:** working diff for the server bridge contract test replacement and docs references.
- **Files changed:** deleted `tools/reference-host/test/server-bridge-contract.test.js`, added `tools/reference-host/test/forge-server-bridge-contract.test.js`, updated `docs/08_TEST_PLAN.md` and `docs/13_EXAMPLE_APP_COVERAGE.md`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --test --no-warnings tools/reference-host/test/forge-server-bridge-contract.test.js` -> failed in sandbox with `listen EPERM`; rerun unsandboxed -> passed; `node --no-warnings tools/check-repo.mjs` -> passed.

## Findings

No blocking findings.

- [P2] The Forge contract covers CoreCommand HTTP behavior, not the old generated-webapp package/control fixture matrix. Resolution: acceptable for this cutover step because the old matrix targets the retired v0.4 server surface; compatibility expansion belongs to future Forge server product decisions.
- [P3] The test needs local TCP bind permission. Resolution: sandboxed run failed as expected; exact test passed unsandboxed.

## Resolution status

- The new test starts `forge-server`, waits for `/health`, posts a serialized `CoreCommand` to `/bridge`, verifies malformed command handling, drains `/events/drain`, and confirms old `/control/command` is not silently accepted.
- The old Zig server build-and-fixture contract test is removed.
- Docs now point to `forge-server-bridge-contract.test.js`.

## Follow-ups

- Add richer Forge server route coverage as `/control` compatibility or a new Forge-native control surface is designed.
- Keep native-host bridge tests separate from server bridge tests; they still assert legacy `core.step` until host ports land.
