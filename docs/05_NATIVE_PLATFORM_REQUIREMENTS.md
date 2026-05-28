# Native Platform Requirements

This document is normative for **v0.4**. Each native host must satisfy the bridge contract in docs/03 and the storage contract in docs/27/28.

## 1. Shared platform contract **[v0.1]**

Every platform shell must implement the same bridge dispatch contract.

Required native services:

- `core.step`
- `storage.get`
- `storage.set`
- `storage.remove`
- `storage.list`
- `dialog.openFile`
- `dialog.saveFile`
- `notification.toast`
- `network.request`
- `app.log`
- `runtime.capabilities` (v0.3)

If a method is not available on a platform during early implementation, it must return:

```json
{
  "code": "platform_unsupported",
  "message": "dialog.saveFile is not implemented on this platform yet"
}
```

Do not silently no-op except for `app.log` (which may discard `debug` entries in production).

### 1.1 Bridge channel rules (cross-platform) **[v0.1]**

Every native bridge implementation must:

1. Identify the calling frame by host-side metadata (origin, frame id, message port). Never trust an `appId` field supplied in the request payload — the runtime derives `appId` from its per-mount nonce (docs/03 §2.1) and the native bridge cross-checks.
2. Parse JSON strictly. Reject extra top-level fields with `invalid_request`.
3. Apply permission checks on the **native side too**, not only the web runtime.
4. Normalize all errors to the schema in docs/03 §5.
5. Avoid exposing raw native objects (file handles, file paths, native pointers) to JavaScript.
6. Avoid synchronous blocking calls from the WebView thread where possible.

## 2. iOS shell **[v0.1]**

Technology:

- Swift, iOS 16+.
- `WKWebView` with `WKWebViewConfiguration` set to a non-persistent data store for sandboxed apps.
- Static Zig library for iOS arm64 simulator and device builds.
- SQLite via the system framework or `GRDB.swift`.

Required behavior:

- Load runtime from bundled resources via `loadFileURL:allowingReadAccessToURL:`.
- Bridge: `WKScriptMessageHandlerWithReply` (iOS 14+) so calls return a `Promise`-shaped reply. Each mounted app frame gets its own handler name tied to its `mount_token`.
- Persist storage under Application Support → SQLite (`PlatformDatabase`).
- File open/save via `UIDocumentPickerViewController`.
- Toast: native overlay (UIKit) or runtime UI fallback.
- Network requests through `URLSession`, with `manifest.networkPolicy` enforced before the request leaves the process.
- Zig core via static library.

Distribution constraints (App Store Guideline 4.7 — see docs/00 D1):

- App Store builds may distribute only the 5 first-party bundled mini-apps.
- Sideloaded / TestFlight builds may install AI-generated packages with the full G7 control plane.
- Implement Guideline 4.7.4 mini-app index and 4.7.5 age gating when bundled distribution is enabled.

Acceptance:

- iOS simulator launches.
- Runtime launcher displays.
- All 5 example apps load.
- `core.step` returns a real Zig response.
- Storage survives app restart (SQLite-backed).
- App-store-distributed build refuses to install non-bundled packages.

## 3. macOS shell **[v0.1]**

Technology:

- Swift, macOS 13+.
- `WKWebView`.
- Static or dynamic Zig library.
- SQLite (`GRDB.swift` or system).

Required behavior:

- Load runtime from bundled resources.
- Native open/save dialogs via `NSOpenPanel` / `NSSavePanel`.
- Storage under `~/Library/Application Support/<bundle id>/` → SQLite.
- Network requests through `URLSession`.
- Native menu exposes reload/debug actions in dev builds.
- Dev control plane bound to `127.0.0.1` in dev builds.

Acceptance:

- macOS app launches from Xcode or command line.
- Runtime loads examples.
- Dialog APIs function.
- Zig core bridge functions.

## 4. Android shell **[v0.1]**

Technology:

- Kotlin, minSdk 26 (Android 8.0), targetSdk 35.
- Android WebView with AndroidX Webkit (`androidx.webkit:webkit:1.12.0` or later).
- JNI wrapper for Zig shared library.
- SQLite via `androidx.sqlite` (no Room required at this layer).

Bridge mechanism — **required**:

- Use `WebViewCompat.addWebMessageListener(webView, "AppRuntimeBridge", allowedOriginSet, listener)` after `WebViewFeature.isFeatureSupported(WEB_MESSAGE_LISTENER)` returns true.
- `allowedOriginSet` must be the single internal origin used to serve the runtime (`https://runtime.local.platform` or equivalent). No wildcards.
- The listener receives `sourceOrigin` and `isMainFrame`, both of which must be verified before dispatch.
- `addJavascriptInterface` is **forbidden** for the production bridge. It cannot verify caller origin and has historical RCE vectors. Static analysis must fail builds that call it on the runtime WebView.

WebView hardening (mandatory):

- `settings.javaScriptEnabled = true` (required for the runtime, no exceptions).
- `settings.allowFileAccess = false`.
- `settings.allowFileAccessFromFileURLs = false`.
- `settings.allowUniversalAccessFromFileURLs = false`.
- `WebView.setWebContentsDebuggingEnabled(false)` in release builds.
- `setSafeBrowsingEnabled(true)` in release builds.
- Serve runtime via `WebViewAssetLoader` so all requests use a known internal HTTPS origin.

Required behavior:

- Load runtime from `assets/runtime/index.html` via `WebViewAssetLoader`.
- Store app data via `PlatformDatabase` (SQLite) in app-private storage.
- File open/save through `ActivityResultContracts.OpenDocument` / `CreateDocument`.
- Network requests through OkHttp.
- Toast through Android `Toast`/`Snackbar` or runtime fallback.

Acceptance:

- Android emulator launches.
- Runtime loads examples through `WebMessageListener`.
- Build fails if `addJavascriptInterface` is referenced in the bridge code.
- JNI core bridge functions for arm64 and x86_64 debug builds.
- Storage persists across restart (SQLite).

## 5. Windows shell **[v0.1]**

Technology:

- C++/WinRT.
- WebView2 (Edge Chromium, runtime version 1.0.2592 or later).
- Zig dynamic library or static library.
- SQLite (`winsqlite3.dll` or vendored).

Required behavior:

- Load runtime from packaged resources via `SetVirtualHostNameToFolderMapping`.
- Bridge: `WebMessageReceived` with origin check against the virtual host.
- Store app data under `%LOCALAPPDATA%\<product>\` → SQLite.
- File dialogs via Win32 common dialog APIs.
- Network requests via WinHTTP or libcurl.
- Toast may be runtime UI fallback in v0.1.

Acceptance:

- Windows app launches.
- WebView2 initializes and loads runtime from virtual host.
- Bridge dispatch works.
- Zig core bridge functions.

## 6. Linux shell **[v0.1]**

Technology:

- C, GTK4, WebKitGTK 2.40+.
- Zig dynamic library.
- SQLite (system).

Required behavior:

- Load runtime from installed resources via `webkit_web_view_load_uri` with a `webkit_security_manager_register_uri_scheme_as_secure` custom scheme.
- Bridge: `WebKitUserContentManager` script-message handler per mounted app.
- Store app data under `$XDG_DATA_HOME/<product>/` → SQLite.
- File dialogs via GTK `GtkFileDialog`.
- Network requests via libsoup (`SoupSession`).
- Toast can be runtime UI fallback.

Acceptance:

- Linux app launches on Ubuntu-like target.
- Runtime loads examples.
- Bridge dispatches calls.
- Zig core bridge functions.

## 7. Server **[v0.1]**

Technology:

- Zig executable.
- Direct import of Zig core, no FFI needed.
- SQLite for development, Postgres-compatible logical schema for production (docs/27).

Required behavior:

- Expose minimal HTTP endpoints for the bridge contract.
- Use the same core event/action contract as native apps.
- SQLite-backed storage in dev; Postgres in production.

Suggested endpoints:

```text
GET  /health
POST /bridge                # generic bridge dispatch; body is a docs/01 §7 request
POST /core/step             # convenience wrapper around POST /bridge
GET  /webapps/examples
POST /webapps/validate
```

The control plane (docs/14) is a separate HTTP surface (`/control/*`) gated by the token described in docs/14 §Authentication.

Acceptance:

- `zig build run-server` starts.
- `/health` returns OK.
- `/bridge` matches the contract fixtures byte-for-byte against the fake host.
- Invalid requests return structured errors.

## 8. Platform build artifacts **[v0.1]**

Zig core build outputs:

```text
ios-arm64-device/libzig_core.a
ios-arm64-simulator/libzig_core.a
macos-arm64/libzig_core.a
macos-x86_64/libzig_core.a
android-arm64-v8a/libzig_core.so
android-x86_64/libzig_core.so
windows-x86_64/zig_core.dll + zig_core.lib
linux-x86_64/libzig_core.so
```

The Zig toolchain version must be pinned in `zig-core/build.zig.zon` to keep determinism tests reproducible.

## 9. Shared generated resources **[v0.1]**

The runtime and examples are copied into each native app bundle/assets at build time.

```text
runtime/
examples/                   # canonical source: webapps/examples/ (docs/02)
  notes-lite/
  task-workbench/
  file-transformer/
  api-dashboard/
  core-replay-lab/
```

## 10. Platform database requirements **[v0.4]**

Every native host must include a `PlatformDatabase` module with these responsibilities.

| Platform | DB requirement |
|---|---|
| iOS | SQLite in Application Support; migrations run on launch |
| macOS | SQLite in Application Support; migrations run on launch |
| Android | SQLite database in app-private storage; migrations run before WebView runtime loads |
| Windows | SQLite under LocalAppData; migrations run on launch |
| Linux | SQLite under XDG data home; migrations run on launch |
| Fake host | SQLite in-memory by default, file-backed when requested |
| Server | SQLite for dev, Postgres-compatible logical schema for production |

Native hosts must implement:

- `PlatformDatabase.open()` (with integrity check; fall back to backup-restore on corruption);
- `PlatformDatabase.migrate()` (idempotent; applies all `db/sqlite/*.sql` in order);
- transaction helper that exposes `BEGIN`/`COMMIT`/`ROLLBACK` to the runtime bridge dispatcher;
- package registry repository;
- app storage repository;
- runtime/debug log repository;
- backup export/import repository;
- safe DB query endpoints for the dev control plane.

Generated apps must not know that SQLite exists. The runtime never returns DB handles, paths, or row ids to the app frame.

## 11. WebView crash and resource exhaustion behavior **[v0.3]**

Each native host must implement:

- **Crash recovery.** If the WebView process is killed, the host shows a re-mount banner, persists the last `runtime_session` row, and offers a "reload" action. Auto-remount is allowed only when the previous mount completed `runtime.ready`.
- **Quota exhaustion.** When a budget is exceeded (`RESOURCE_BUDGET_EXCEEDED`), the host counts the violation. After 3 violations within 60 seconds the runtime quarantines the installed version and reverts to the previous active version.
- **Slow core.** `core.step` calls timing out (default 2000 ms) return `timeout`. The host does not block the WebView thread waiting for the core.
- **Storage failure.** Disk-full or DB corruption returns `storage_error`. The host writes the failure into `app_install_reports` if it occurred during install.

## 12. Compile-out rules for production builds **[v0.2]**

Production native builds must:

- Compile out the dev control plane HTTP server (docs/14).
- Compile out `runtime.unsafe_eval` and `runtime.unsafe_sql`.
- Reject any `--control-plane-port` / `--allow-runtime-mismatch` command-line flag with a fatal error and a logged audit entry.
- Refuse `algorithm = "none-dev"` signatures (docs/17).
