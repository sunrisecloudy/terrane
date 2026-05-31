# Linux Host Target

Current implementation status: partial.

The scaffold is a C GTK4/WebKitGTK host using JSON-GLib and SQLite. It follows the same bridge boundary as the reference host and native scaffolds:

```text
meson.build
src/main.c
src/webkit_host.c
src/web_bridge.c
src/zig_core_bridge.c
src/platform_storage.c
src/platform_dialogs.c
src/platform_notifications.c
src/platform_network.c
resources/runtime/
resources/examples/
```

Implemented now:

- Creates a GTK application window with a WebKitGTK runtime view.
- Registers `app-runtime` as a secure/CORS-enabled custom scheme and loads the runtime plus app-scoped generated app resources through it, preferring packaged `resources/runtime` and `resources/webapps/examples` beside the executable before falling back to the repo layout.
- Injects the native `AppRuntime` bootstrap into app-scoped WebKit frames so generated app scripts run from their package URL instead of `srcdoc`.
- Receives runtime bridge payloads through reply-capable `WebKitUserContentManager` script-message handling.
- Handles runtime-owned `{ appId, mountToken, request }` bridge envelopes and derives app context from the envelope on the host side.
- Applies native-side permission checks before dispatching bridge calls.
- Persists `storage.*` through SQLite `app_storage(app_id, key, value_json)`.
- Validates `notification.toast` `message`/`level` params against the bridge contract.
- Implements native `dialog.openFile` and `dialog.saveFile` through owner-bound GTK file chooser native dialogs.
- Implements `network.request` through libsoup with manifest `networkPolicy` checks.
- Loads SQLite migrations from packaged `resources/db/sqlite` before falling back to checked-in migrations.
- Loads `libzig_core.so` through `dlopen` for `core.step`, using `NATIVE_AI_ZIG_CORE_SO` first, then the packaged library beside `native-ai-webapp-host`, then repo-local/install candidate paths.
- Reports `core.step` in `runtime.capabilities` from the actual Zig library load status and returns structured `platform_unsupported` when the library is absent.
- Starts a debug-build-only loopback dev control plane when `--native-ai-dev-control` or `NATIVE_AI_LINUX_DEV_CONTROL=1` is set, writes a per-launch `0600` control token, token-gates `GET /health` plus session create/snapshot/events/capabilities/command/end routes, supports `platform.list_targets`, `platform.list_webapps`, static HTML `runtime.screenshot` / `runtime.query` / target interaction / wait / timer / visible-text assertion controls, static accessibility snapshot/audit/assertion controls over bundled app packages, permission-checked `runtime.call_bridge` / `runtime.core_step`, direct `runtime.storage_get` / `runtime.storage_set`, confirmation-gated `runtime.storage_reset` / `platform.reset_webapp` with pre-reset snapshots, `runtime.assert_storage`, DB-backed `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.network_mock_set`, `runtime.network_mock_reset`, `runtime.dialog_mock_set`, and one-shot `runtime.fault_inject`, plus safe `db.snapshot`, fixed `db.query_*`, hashed `db.export_backup` / `db.import_backup`, and `db.export_debug_bundle` inspection/export commands, and audits accepted/rejected requests to SQLite.

Release packaging for the Linux host runs on Linux/x64:

```sh
node --no-warnings tools/package-release.mjs --out artifacts --build-native-linux
```

The artifact is staged at `artifacts/native-apps/linux/linux-x86_64/NativeAIWebappHost/` with `native-ai-webapp-host`, `libzig_core.so`, runtime resources, example app packages, SQLite migrations, and hashed entries in `release-manifest.json`.

## Docker smoke

The Linux host can be built and smoke-tested from any Docker-capable development machine, including macOS:

```sh
node --no-warnings tools/run-linux-native-docker.mjs
```

The helper builds `native/linux/Dockerfile`, installs GTK4/WebKitGTK/SQLite/Meson/Zig 0.15.2 dependencies, mounts the repo read-only at `/workspace`, and runs the runtime smoke, token-gated target/webapp listing smoke, static HTML runtime UI control smoke, static accessibility snapshot/audit/assertion smoke, safe DB inspection smoke, hashed debug-bundle and portable backup export/import checks, explicit app-storage snapshot create/restore/compare checks, direct storage get/set/assert/reset controls, DB-backed bridge call inspection/assertions, log clearing, notification capture, no-console-error assertions, DB-backed core replay/snapshot/action assertions, DB-backed network/dialog mock bridge calls, arbitrary-SQL rejection check, and release-build production-guard audit check:

```sh
NATIVE_AI_LINUX_SMOKE_LAUNCH=1 node --test --no-warnings tools/reference-host/test/linux-native-build.test.js
```

Useful options:

```sh
node --no-warnings tools/run-linux-native-docker.mjs --platform linux/amd64
node --no-warnings tools/run-linux-native-docker.mjs --skip-build
node --no-warnings tools/run-linux-native-docker.mjs --build-only
node --no-warnings tools/run-linux-native-docker.mjs --dry-run
```

MVP acceptance:

- Launches on Linux with GTK4/WebKitGTK installed.
- Loads runtime and examples from installed resources.
- Implements storage under XDG data path.
- Loads `libzig_core.so`.
- Implements `core.step`.


## Dev control plane

This host must support a dev/test-only control plane for Codex micro-testing.

Required behavior:

- Enable only in debug/dev builds. Linux currently implements token-gated `GET /health`, session lifecycle routes, minimal DB-backed snapshots/events/capabilities, `platform.health`, `platform.list_targets`, `platform.list_webapps`, static HTML `runtime.screenshot`, `runtime.query`, target interaction, `runtime.wait_for`, `runtime.timer_advance`, `runtime.assert_visible`, and `runtime.assert_text` controls, static `runtime.accessibility_snapshot`, `runtime.run_accessibility_audit`, and `runtime.assert_accessibility` controls over bundled app packages, static bundled `runtime.run_smoke_tests` with `micro_tests`/`test_runs` persistence, direct storage get/set/assert/reset controls, DB-backed `platform.create_snapshot`, `platform.restore_snapshot`, `runtime.compare_snapshot`, `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, `runtime.assert_no_console_errors`, `runtime.network_mock_set`, `runtime.network_mock_reset`, `runtime.dialog_mock_set`, `runtime.fault_inject`, `runtime.core_snapshot`, `runtime.replay_events`, and `runtime.assert_core_action`, permission-checked `runtime.call_bridge` / `runtime.core_step` session commands through the native bridge, and safe `db.snapshot`, fixed `db.query_*`, hashed `db.export_backup` / `db.import_backup`, plus `db.export_debug_bundle` inspection/export commands without arbitrary SQL.
- Require a random control token. Linux writes it to `$XDG_RUNTIME_DIR/native-ai-webapp/control.token` unless `PLATFORM_CONTROL_TOKEN_FILE` is set.
- Expose host/runtime/session state through the control protocol and route bridge-driving session commands through the same manifest-derived sandbox context as the WebKit runtime.
- Route UI control, bridge inspection, storage mocks, network mocks, dialog mocks, and replay operations to the runtime.
- Compile out or hard-disable the control plane in production/release builds.

See `docs/14_CODEX_CONTROL_PLUGIN.md` and `devtools/control-plane/README.md`.

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/reference hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. Linux runs migrations, persists app registry/package/storage/log/test records, and exposes safe DB inspection through the dev control plane.
