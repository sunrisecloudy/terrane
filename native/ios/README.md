# iOS Host Target

Current implementation status: partial.

The scaffold is a SwiftPM/UIKit/WKWebView host module that mirrors the macOS bridge shape while staying iOS-specific:

```text
App.swift
WebHostView.swift
WebBridge.swift
ZigCoreBridge.swift
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
- Applies native-side permission checks before dispatching bridge calls.
- Persists `storage.*` through SQLite `app_storage(app_id, key, value_json)`.
- Implements `network.request` through ephemeral `URLSession` with manifest `networkPolicy` checks.
- Returns structured `platform_unsupported` responses for dialogs and Zig core until those services are wired.

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

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/fake hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
