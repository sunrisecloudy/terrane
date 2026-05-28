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
- Handles unsupported dialog methods with structured errors until fully implemented.

Current implementation status:

- Gradle Android/Kotlin project scaffold exists under `app/`.
- Runtime loads through `WebViewAssetLoader` on the appassets origin.
- Bridge uses `WebViewCompat.addWebMessageListener` with an origin allowlist.
- SQLite-backed `storage.*` uses host-derived app context and storage-prefix checks.
- Native bridge applies manifest-style permission checks before dispatch.
- Dialog, network, and Zig core paths currently return structured `platform_unsupported`.

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
