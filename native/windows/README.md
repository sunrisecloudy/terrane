# Windows Host Target

Current implementation status: partial.

The scaffold is a C++/WinRT desktop host using WebView2 and `winsqlite3`. It is intentionally parallel to the reference host/native bridge contract:

```text
CMakeLists.txt
src/main.cpp
src/WebViewHost.cpp
src/WebBridge.cpp
src/ZigCoreBridge.cpp
src/PlatformStorage.cpp
src/PlatformDialogs.cpp
src/PlatformNotifications.cpp
src/PlatformNetwork.cpp
src/resources/runtime/
src/resources/examples/
```

Implemented now:

- Initializes WebView2 1.0.2592+ and maps packaged runtime resources through `SetVirtualHostNameToFolderMapping`, with checked-in repo resources as the dev fallback.
- Receives bridge payloads through `WebMessageReceived` and checks the internal runtime origin before dispatch.
- Rejects unknown top-level native bridge request/envelope fields with `invalid_request`.
- Handles runtime-owned `{ appId, mountToken, request }` bridge envelopes and derives app context from the envelope on the host side.
- Applies native-side permission checks before dispatching bridge calls.
- Persists `storage.*` through SQLite `app_storage(app_id, key, value_json)`.
- Implements native `dialog.openFile` and `dialog.saveFile` through owner-bound Win32 common file dialogs.
- Implements `network.request` through WinHTTP with manifest `networkPolicy` checks.
- Loads `zig_core.dll` through `LoadLibraryW` for `core.step`, using `NATIVE_AI_ZIG_CORE_DLL` first, the executable-adjacent packaged DLL next, and repo-local candidate paths as dev fallbacks.
- Reports `core.step` in `runtime.capabilities` from the actual Zig DLL load status and returns structured `platform_unsupported` when the DLL is absent.
- Windows-only release smoke coverage builds the native package, launches it from the staged artifact directory with no `NATIVE_AI_ZIG_CORE_DLL`, and verifies executable-relative runtime/example/SQLite resources plus `zig_core.dll`-backed `core.step`.

MVP acceptance:

- Launches on Windows.
- Initializes WebView2.
- Loads runtime and examples from resources/local files.
- Loads `zig_core.dll`.
- Implements storage and `core.step`.


## Dev control plane

This host must support a dev/test-only control plane for Codex micro-testing.

Implemented now (source/static verified here; runtime smoke is Windows-only):

- Debug builds can enable a loopback-only control listener with `--native-ai-dev-control` or `NATIVE_AI_WINDOWS_DEV_CONTROL=1`.
- `--control-plane-port` / `--control-plane-port=...` selects the listener port; `0` lets Windows assign a free port.
- Each launch writes a fresh random token to `%LOCALAPPDATA%\NativeAIWebappPlatform\control.token`, or `PLATFORM_CONTROL_TOKEN_FILE` when set for tests.
- Requests must send `X-Platform-Control-Token`; missing or invalid tokens return `control_auth_required`.
- `GET /health` returns Windows target health and records accepted/rejected control audit rows in SQLite.
- Session routes create/end control sessions, create linked runtime sessions when `appId` is supplied, and expose DB-backed snapshot/events/capabilities responses.
- `POST /sessions/:id/command` supports `platform.health`, `platform.list_targets`, `platform.list_webapps`, `runtime.capabilities`, `runtime.call_bridge`, and `runtime.core_step`; bridge/core commands are app-bound, reject ended sessions, and dispatch through the native WebBridge on the host thread.
- `POST /sessions/:id/command` supports source/static-verified `runtime.replay_events` with a fresh Zig core replay plus DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`.
- `POST /sessions/:id/command` supports source/static-verified `runtime.storage_get`, `runtime.storage_set`, `runtime.storage_reset` / `platform.reset_webapp`, and `runtime.assert_storage` with native storage-prefix enforcement, confirmation-gated destructive reset, pre-reset runtime snapshots, and `bridge_calls` logging for direct get/set controls.
- `POST /sessions/:id/command` supports DB-backed `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors` inspection/assertion over storage, bridge-call, notification, core-event/action, and `app.log` rows.
- `POST /sessions/:id/command` supports DB-backed `runtime.network_mock_set`, `runtime.network_mock_reset`, and `runtime.dialog_mock_set`; `network.request`, `dialog.openFile`, and `dialog.saveFile` consume matching mock rows before falling back to native effects.
- `POST /sessions/:id/command` supports safe DB inspection through `db.snapshot` and fixed `db.query_app_storage`, `db.query_app_versions`, `db.query_bridge_calls`, `db.query_core_events`, and `db.query_test_runs` commands. These use fixed table/column allowlists and do not expose arbitrary SQL.
- `POST /sessions/:id/command` supports source/static-verified `db.export_backup`, `db.import_backup`, and `db.export_debug_bundle` using fixed table reads/writes, `sha256:` content hashes, transactional import, and persisted `backup_exports` rows.
- Release builds reject dev-control startup flags and environment enablement.

Remaining protocol work:

- UI control and storage mocks are not implemented on Windows yet.

See `docs/14_CODEX_CONTROL_PLUGIN.md` and `devtools/control-plane/README.md`.

## v0.4 persistence requirement

The platform database layer for this target uses SQLite through `PlatformDatabase`, applies packaged or checked-in migrations, runs `PRAGMA integrity_check`, persists app registry/package/storage/log/test records for the implemented bridge/control surfaces, and exposes safe DB inspection plus backup/debug-bundle import/export through the dev control plane. The server supports SQLite in dev and the Postgres-compatible logical schema in production.
