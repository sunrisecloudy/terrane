# iOS Host Skeleton Target

Codex should implement this as a Swift iOS app using `WKWebView`.

Minimum files to create:

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
