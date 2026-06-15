# 029 Live Reference Cleanup

## Slice goal

Remove remaining live references to the retired Zig core/CRDT/server vocabulary outside archived v0.4 docs, and make the Forge server release flag explicit before deletion slices.

## Review mode

Independent Codex self-review. Claude Code review was intentionally not requested because the user instructed this run to proceed independently from Claude Code.

## Files changed

- `.github/workflows/ci.yml`
- `IMPLEMENTATION_STATUS.md`
- `codex/PLATFORM_BOOTSTRAP_TASKS.md`
- `docs/00_V1_PIVOT.md`
- `docs/08_TEST_PLAN.md`
- `docs/10_ACCEPTANCE_CHECKLIST.md`
- `docs/12_RELEASE_AND_CI.md`
- `forge/crates/core/src/commands/legacy_core_step.rs`
- `forge/crates/core/src/commands/sync.rs`
- `tests/fixtures/capabilities/server.json`
- `tools/README.md`
- `tools/check-repo.mjs`
- `tools/package-release.mjs`
- `tools/reference-host/test/native-core-timeout-source.test.js`

## Commands and evidence

- `rg -n "build-server|zig_core|zig-core|libzig_core|zig_crdt|zig-crdt|libzig_crdt|terrane_zig_core_|terrane_zig_crdt_|core_step_json|ZigCoreBridge|setup-zig|mlugg/setup-zig|ZIG_GLOBAL_CACHE|TERRANE_ZIG" README.md CONTRIBUTION.md IMPLEMENTATION_STATUS.md docs codex forge tools native windom-plan.md .github/workflows` now returns only archived v0.4 docs.
- `rg -n "zig-server|Zig Server|Zig server|Zig core|Zig-core|zig-core|zig_crdt|zig-crdt|libzig_core|core_step_json|build-server|ZigCoreBridge|TERRANE_ZIG" IMPLEMENTATION_STATUS.md docs/08_TEST_PLAN.md docs/10_ACCEPTANCE_CHECKLIST.md tests/fixtures/capabilities tools/check-repo.mjs tools/package-release.mjs .github/workflows/ci.yml tools/README.md forge/crates/core/src/commands` returned no live hits.
- `node --test --no-warnings tools/reference-host/test/release-packaging.test.js` passed; macOS/Linux/Windows native artifact tests skipped on this host as platform-specific or because `hdiutil create` is unavailable.
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js` passed.
- `node --test --no-warnings tools/reference-host/test/runtime-capabilities-contract.test.js` passed.
- `node --no-warnings tools/check-repo.mjs` passed.
- `git diff --check` passed.

## Findings

- No blocking correctness findings in the cleanup diff.
- The old `server/` directory is still route-richer than `forge-server`, but read-only server analysis found no current live build, tool, CI, package, or check path that reads or builds `server/`. Deleting it should be recorded as retiring the unused v0.4 HTTP surface, not as claiming strict route parity.
- The stale `tests/server/server-api-smoke.md` fixture remains a deletion-slice candidate with `server/`.

## Resolution

- Renamed the release-packaging CLI flag from `--build-server` to `--build-forge-server`.
- Removed `tools/check-repo.mjs` dependency on `server/src/main.zig`.
- Repointed live status/test docs and the server capability fixture to the Forge server/CoreCommand surface.
- Left archived v0.4 docs intact.
