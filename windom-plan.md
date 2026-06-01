# Windows Continuation Plan

This note captures the remaining non-CRDT work to continue on a Windows machine.

## Current state

- The worktree was clean on `main` before this note was added.
- Linux package lifecycle control routes are committed and Docker-smoke verified.
- Windows has broad source/static coverage, but the main Windows runtime acceptance rows remain unchecked until run on Windows.
- CRDT/notebook work is intentionally excluded here because another agent owns it.

## Windows acceptance still unchecked

From `docs/10_ACCEPTANCE_CHECKLIST.md`:

- Windows app launches.
- WebView2 loads runtime.
- Bridge works.
- Zig DLL loads.
- Storage persists.
- Debug dev control plane runtime-smoke verifies per-launch token file, loopback bind, token-gated `GET /health` plus session create/snapshot/events/capabilities/command/end routes, and accepted/rejected audit rows.
- Debug dev control plane runtime-smoke verifies `runtime.capabilities`, `runtime.call_bridge`, and `runtime.core_step` through permission-checked bridge dispatch with bridge/core DB logging.
- Debug dev control plane runtime-smoke verifies safe `db.snapshot` and fixed `db.query_*` inspection without arbitrary SQL.

## First Windows verification command

Run on a Windows machine with WebView2/CMake/native build dependencies available:

```powershell
$env:NATIVE_AI_WINDOWS_SMOKE_LAUNCH = "1"
node --test --no-warnings tools/reference-host/test/windows-native-build.test.js
```

If the packaged artifact smoke is separate in your environment, also run the release/package flow that stages `resources/runtime`, `resources/webapps/examples`, `resources/db/sqlite`, and `zig_core.dll`, then launch without `NATIVE_AI_ZIG_CORE_DLL`.

## Likely Windows implementation gaps

These were identified from source/status review and should be checked before marking acceptance rows:

- Installed app runtime mounting is still likely example-only. `native/windows/src/WebViewHost.cpp` gates resource mounting around known bundled example IDs, while installed package rows live in SQLite `app_files`.
- Static dev-control HTML helpers in `native/windows/src/DevControlPlane.cpp` use bundled app HTML helpers for many commands.
- Docs/14 command-form routes appear missing or need explicit structured support:
  - `platform.launch`
  - `platform.stop`
  - `platform.reload_runtime`
  - `runtime.snapshot`
  - `platform.run_repair_loop`
- Token hardening needs confirmation:
  - current-user-only ACL on the token file
  - temporary ban after repeated auth failures
- Bridge wrong-channel errors should align with `bridge.unauthorized_channel`.
- `runtime.capabilities` shape should be compared against the reference-host contract with only documented platform overrides.

## Suggested order

1. Run the Windows native smoke command above and record the exact failing rows.
2. Fix launch/WebView2/runtime-load first, because it unlocks the rest of the runtime acceptance.
3. Verify bridge/storage/Zig DLL through the real WebView2 runtime path.
4. Extend Windows smoke assertions for dev-control auth/session/audit, `runtime.capabilities`, `runtime.call_bridge`, `runtime.core_step`, and safe DB inspection.
5. Only check Windows rows in `docs/10_ACCEPTANCE_CHECKLIST.md` after real Windows runtime evidence exists.
6. Update `IMPLEMENTATION_STATUS.md` in the same commit as any newly verified Windows acceptance.

## Non-Windows follow-up

- Linux installed-package WebKit mounting from SQLite `app_files` is still follow-up work, but it can be continued on macOS via Docker later.
- CRDT/notebook verification remains owned by the other agent.
