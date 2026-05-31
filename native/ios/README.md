# iOS Host Target

Current implementation status: partial.

The scaffold is a SwiftPM/UIKit/WKWebView host module that mirrors the macOS bridge shape while staying iOS-specific:

```text
App.swift
WebHostView.swift
WebBridge.swift
ZigCoreBridge.swift
CZigCoreBridge/
PlatformStorage.swift
PlatformDialogs.swift
PlatformNotifications.swift
PlatformNetwork.swift
Resources/runtime/
Resources/examples/
```

Implemented now:

- Creates a `WKWebView` and registers `NativeAIPlatformBridge` through `WKScriptMessageHandlerWithReply`.
- Uses a non-persistent web data store for the runtime WebView.
- Derives `appId` and storage prefix from the sandbox frame URL instead of request bodies.
- Accepts runtime-owned bridge envelopes from the main WKWebView frame and derives native permissions/policy from the envelope app id.
- Applies native-side permission checks before dispatching bridge calls.
- Persists `storage.*` through SQLite `app_storage(app_id, key, value_json)`.
- Implements `network.request` through ephemeral `URLSession` with manifest `networkPolicy` checks.
- Loads statically linked Zig core symbols when present and falls back to `libzig_core.dylib` for simulator/dev `core.step`, using `NATIVE_AI_ZIG_CORE_DYLIB` first.
- Reports `core.step` in `runtime.capabilities` from the actual Zig core link/load status and returns structured `platform_unsupported` when unavailable.
- Implements native `dialog.openFile` and `dialog.saveFile` through `UIDocumentPickerViewController` with asynchronous bridge replies.
- Includes a DEBUG simulator-only loopback dev-control first slice with per-launch token-file auth, a token-gated `GET /health` endpoint, lightweight session/control routes, and SQLite `control_sessions` / `control_commands` auditing when launched with `--native-ai-dev-control`.
- Debug simulator smoke can verify all five bundled example app ids through host-derived `runtime.capabilities` bridge dispatch.
- Debug simulator smoke verifies native storage reset creates a manual pre-reset `runtime_snapshots` row and clears storage through the real bridge.

MVP acceptance:

- Launches in iOS simulator.
- Loads `runtime/index.html` from app resources.
- Lists and launches all five bundled example apps.
- Routes `AppRuntime.call` requests through Swift bridge.
- Implements storage and `core.step`.
- Returns structured errors for unsupported methods.


## Dev control plane

This host must support a dev/test-only control plane for Codex micro-testing.

Required behavior:

- Enable only in debug/dev builds.
- Require a random control token.
- Expose host/runtime/session state through the control protocol.
- Route UI control, bridge inspection, storage mocks, network mocks, dialog mocks, and replay operations to the runtime.
- Compile out or hard-disable the control plane in production/release builds.

See `docs/14_CODEX_CONTROL_PLUGIN.md` and `devtools/control-plane/README.md`.

Implemented first slice:

- DEBUG simulator-only `IOSDevControlPlane` starts from `--native-ai-dev-control` or `NATIVE_AI_IOS_DEV_CONTROL=1`, binds to `127.0.0.1`, writes a 0600 token file under Application Support by default, serves token-gated `GET /health`, records accepted/rejected control audit rows in SQLite, and exposes lightweight `/control/sessions` plus `/control/command` handlers for `platform.list_targets`, `platform.list_webapps`, bridge-routed `runtime.capabilities`, `runtime.call_bridge`, `runtime.core_step`, `runtime.storage_get`, `runtime.storage_set`, `runtime.assert_storage`, confirmation-gated `runtime.storage_reset` / `platform.reset_webapp` with runtime-log cleanup for platform reset, DB-backed `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors`, fresh-core `runtime.replay_events`, DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`, explicit `platform.create_snapshot`, confirmation-gated `platform.restore_snapshot`, normalized `runtime.compare_snapshot`, safe `db.snapshot`, fixed `db.query_*`, and `db.export_debug_bundle`; the safe DB tools are also available through token-gated `/db/*` and `/control/db/*` routes.

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/reference hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
