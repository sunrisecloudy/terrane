# Windows Host Target

Current implementation status: partial.

The scaffold is a C++/WinRT desktop host using WebView2 and `winsqlite3`. It is intentionally parallel to the fake host/native bridge contract:

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

- Initializes WebView2 and maps the repo runtime through `SetVirtualHostNameToFolderMapping`.
- Receives bridge payloads through `WebMessageReceived` and checks the internal runtime origin before dispatch.
- Handles runtime-owned `{ appId, mountToken, request }` bridge envelopes and derives app context from the envelope on the host side.
- Applies native-side permission checks before dispatching bridge calls.
- Persists `storage.*` through SQLite `app_storage(app_id, key, value_json)`.
- Implements native `dialog.openFile` and `dialog.saveFile` through owner-bound Win32 common file dialogs.
- Implements `network.request` through WinHTTP with manifest `networkPolicy` checks.
- Loads `zig_core.dll` through `LoadLibraryW` for `core.step`, using `NATIVE_AI_ZIG_CORE_DLL` first and then repo-local/executable candidate paths.
- Reports `core.step` in `runtime.capabilities` from the actual Zig DLL load status and returns structured `platform_unsupported` when the DLL is absent.

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
