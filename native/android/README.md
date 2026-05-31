# Android Host Target

Codex should implement this as a Kotlin Android app using Android WebView.

Minimum files to create:

```text
settings.gradle.kts
build.gradle.kts
app/build.gradle.kts
app/src/main/AndroidManifest.xml
app/src/main/java/.../MainActivity.kt
app/src/main/java/.../WebBridge.kt
app/src/main/java/.../ZigCoreBridge.kt
app/src/main/java/.../PlatformStorage.kt
app/src/main/java/.../PlatformDialogs.kt
app/src/main/java/.../PlatformNotifications.kt
app/src/main/java/.../PlatformNetwork.kt
app/src/main/cpp/zig_core_jni.cpp
app/src/main/assets/runtime/
app/src/main/assets/examples/
app/src/main/jniLibs/arm64-v8a/libzig_core.so
app/src/main/jniLibs/x86_64/libzig_core.so
```

MVP acceptance:

- Launches on Android emulator.
- Loads runtime from assets.
- Lists and launches all five examples.
- Implements storage and JNI `core.step`.
- Implements native file open/save dialogs through Android activity result contracts.

Current implementation status:

- Gradle Android/Kotlin project scaffold exists under `app/`.
- Gradle syncs the shared `runtime-web/` and `webapps/` trees into generated Android assets.
- Runtime loads through `WebViewAssetLoader` on the appassets origin, with the shared runtime and example packages served from packaged assets.
- Bridge uses `WebViewCompat.addWebMessageListener` only after `WebViewFeature.WEB_MESSAGE_LISTENER` support is confirmed, with a single appassets origin allowlist and runtime-owned `{ appId, mountToken, request }` envelope handling.
- WebView setup disables file access, file-URL access, universal file-URL access, and content access for the runtime WebView; release debugging is disabled through the build-mode flag and Safe Browsing is enabled.
- SQLite-backed `storage.*` uses host-derived app context and storage-prefix checks.
- The default sandbox context derives permissions, storage prefix, and network policy from the bundled app manifest instead of hardcoded bridge permissions.
- Native bridge applies manifest-style permission checks before dispatch.
- `network.request` uses OkHttp with manifest `networkPolicy` checks, explicit redirect validation, and request/policy timeout clamping.
- `dialog.openFile` and `dialog.saveFile` use `ActivityResultContracts.OpenDocument`, `OpenMultipleDocuments`, and `CreateDocument` with asynchronous bridge replies.
- `core.step` uses a JNI wrapper that loads packaged `libzig_core.so` and calls the shared Zig C ABI.
- `runtime.capabilities` reports `core.step` from actual JNI/Zig core availability and returns structured `platform_unsupported` when unavailable.
- Debug builds start a loopback-only dev control first slice with a private per-launch `control.token`, SQLite-audited `control_sessions` / `control_commands`, token-gated `/health`, `/control/sessions`, and `/control/command` routes, `platform.list_targets` / `platform.list_webapps` with bundled app metadata, bridge-routed `runtime.capabilities` / `runtime.call_bridge` / `runtime.core_step`, direct `runtime.storage_get` / `runtime.storage_set`, `runtime.assert_storage`, confirmation-gated `runtime.storage_reset` / `platform.reset_webapp` with pre-reset snapshots, DB-backed `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors`, plus allowlisted safe DB inspection commands.

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

Implement the platform database layer for this target. Native/reference hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
