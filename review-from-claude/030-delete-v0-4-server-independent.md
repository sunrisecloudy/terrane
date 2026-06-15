# 030 Delete v0.4 Server

## Slice goal

Delete the retired v0.4 Zig HTTP server and its dedicated mdok smoke fixture after live build, CI, packaging, check, and reference-host paths were repointed to Forge server.

## Review mode

Independent Codex self-review. Claude Code review was intentionally not requested because the user instructed this run to proceed independently from Claude Code.

## Files changed

- `server/README.md`
- `server/build.zig`
- `server/src/main.zig`
- `tests/server/server-api-smoke.md`
- `IMPLEMENTATION_STATUS.md`
- `codex/PLATFORM_BOOTSTRAP_TASKS.md`
- `docs/08_TEST_PLAN.md`
- `tests/fixtures/bridge/valid-runtime-capabilities.json`

## Deletion gate

The deleted server was not removed as route-compatible parity. It was removed because the v0.4 HTTP server surface is no longer a live consumer: release packaging, CI, static checks, public-contract export, and reference-host server tests now target `forge-server`.

Strict old route parity remains intentionally out of scope for this deletion. `forge-server` currently exposes the Forge v1 CoreCommand HTTP spine: `GET /health`, `POST /bridge`, and `POST /events/drain`.

## Zero-reference proof

No non-archived live hits after deletion:

```sh
rg -n --hidden -g '!.git/**' -g '!external-lib/**' -g '!forge/target/**' -g '!target/**' -g '!server/**' -g '!docs/**' -g '!review/**' -g '!review-from-claude/**' -g '!task-between-claude-and-codex/**' -g '!task-jun-15/**' "server/src/main\\.zig|server/README\\.md|working-directory: server|zig build run-server|run-server|TERRANE_SERVER|server-platform\\.sqlite|server-api-smoke|tests/server|zig-server|Zig Server API Smoke|/core/step" .
```

Archived v0.4 docs still mention the old routes and are intentionally retained.

## Commands and evidence

- `node --test --no-warnings tools/reference-host/test/forge-server-build.test.js tools/reference-host/test/forge-server-bridge-contract.test.js tools/reference-host/test/runtime-capabilities-contract.test.js tools/reference-host/test/bridge-fixtures.test.js` passed except the listener test hit sandbox `listen EPERM`.
- `node --test --no-warnings tools/reference-host/test/forge-server-bridge-contract.test.js` passed with escalation so it could bind `127.0.0.1`.
- `node --no-warnings tools/check-repo.mjs` passed.
- `git diff --check` passed.

## Findings

- No live consumer of `server/` remained outside archived v0.4 docs.
- The dedicated `tests/server/server-api-smoke.md` fixture only targeted the deleted v0.4 server and was removed with this slice.
- The bridge runtime-capabilities fixture kept a `server` platform bucket for fixture shape compatibility but now identifies it as `forge-server`.

## Resolution

- Deleted `server/` and `tests/server/server-api-smoke.md`.
- Removed their rows from `IMPLEMENTATION_STATUS.md`.
- Repointed the live test plan and bootstrap notes to `/bridge` and `/events/drain`.
