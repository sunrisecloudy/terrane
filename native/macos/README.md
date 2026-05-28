# macOS Host Target

SwiftPM macOS host using `WKWebView`.

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

- Launches on macOS.
- Loads runtime and all example apps from bundle resources.
- Implements storage, dialogs, network, toast fallback, and `core.step`.
- Exposes a debug reload action during development.

Current local check:

```sh
swift build
```

Current implementation status:

- Launches an AppKit `WKWebView` window.
- Loads `runtime-web/index.html` from the repo checkout for development.
- Defines the native bridge response shape.
- Implements SQLite-backed `storage.*`.
- Implements native open/save dialogs and toast logging.
- Implements `network.request` through ephemeral `URLSession` with manifest `networkPolicy` checks.
- Accepts runtime-owned bridge envelopes from the main WKWebView frame and derives native permissions/policy from the envelope app id.
- Returns structured `platform_unsupported` for unfinished `core.step` wiring.


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
