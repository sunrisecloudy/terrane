# Windows Host Skeleton Target

Codex should implement this as a C++/WinRT desktop app using WebView2.

Minimum files to create:

```text
NativeAIWebappHost.sln
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

MVP acceptance:

- Launches on Windows.
- Initializes WebView2.
- Loads runtime and examples from resources/local files.
- Loads `zig_core.dll`.
- Implements storage and `core.step`.


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
