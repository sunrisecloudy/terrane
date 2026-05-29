# Test Plan: All Levels

## 1. Test strategy summary

The platform has four main test surfaces:

1. **Zig core** — deterministic logic and FFI safety.
2. **Web runtime** — sandbox, permissions, bridge, validators, UI launcher.
3. **Native shells** — WebView loading, native bridge, platform service implementations.
4. **Generated webapps** — package validity, smoke tests, permission behavior, runtime compatibility.

## 2. Test pyramid

```text
High volume:
  Zig unit tests
  JS runtime unit tests
  schema validation tests
  bridge contract tests

Medium volume:
  runtime integration tests
  native bridge integration tests
  webapp smoke tests
  server API tests

Low volume but required:
  full platform E2E tests
  mobile simulator tests
  desktop app launch tests
  security abuse tests
  performance tests
```

## 3. Zig core tests

### Unit tests

- Event parsing accepts valid events.
- Event parsing rejects invalid events.
- Known demo events return expected actions.
- Unknown event behavior is consistent.
- State version increments correctly.
- Replay returns deterministic output.

### FFI tests

- `core_create` returns non-null.
- `core_destroy` handles valid core pointer.
- `core_step_json` returns valid UTF-8 JSON.
- `core_free` releases output buffer.
- Invalid JSON returns structured logical error.
- Oversized input returns safe error.
- Local Zig core build smoke runs with `node --test --no-warnings tools/fake-platform-host/test/zig-core-build.test.js`, which executes `zig test` and builds native static/shared libraries. On macOS it pins `macos.15.0.0` to avoid the local Zig build-runner Darwin 26 linker issue.

### Property/fuzz tests

- Random byte inputs do not crash.
- Random JSON objects either return valid JSON or safe error.
- Replaying same event list twice returns same result.

### Memory tests

- Repeated create/step/free/destroy loop.
- Large output buffer free.
- Error output buffer free.

## 4. Runtime web tests

### Unit tests

- Manifest validator accepts examples.
- Manifest validator rejects missing id/name/version/entry.
- Permission manager maps methods to permissions.
- Permission manager denies unknown methods.
- Storage key checker enforces prefix.
- Quota manager throttles excessive bridge calls.
- Error normalizer returns consistent shape.
- Sandbox manager creates/destroys app contexts.

### Component tests

- Launcher lists installed apps.
- App card opens app.
- Error boundary shows app load failure.
- Debug console records bridge calls.
- Toast display works.

### Integration tests

- Load each example app with mock host.
- App calls storage and receives mocked values.
- App calls core.step and displays result.
- Unknown bridge method is denied.
- Permission-denied call appears in debug console.

## 5. Webapp package tests

For every webapp package:

- Validate manifest schema.
- Validate package file list.
- Scan HTML for banned tags/attributes.
- Scan JS for banned APIs.
- Scan CSS for remote imports.
- Run smoke tests in sandbox with mock bridge.
- Verify app uses only declared permissions.
- Verify storage keys use app prefix.

## 6. Bridge contract tests

### 6.1 Fixture format

Every contract fixture under `tests/fixtures/bridge/` validates against `schemas/bridge-contract-fixture.schema.json` and uses this shape:

```json
{
  "name": "storage.get returns empty for unset key",
  "preconditions": {
    "installApp": "webapps/examples/notes-lite",
    "seedStorage": []
  },
  "request": {
    "method": "storage.get",
    "params": { "key": "notes-lite:notes", "defaultValue": [] }
  },
  "expected": {
    "ok": true,
    "result": { "value": [] }
  },
  "expectedByPlatform": {
    "server": {
      "ok": true,
      "resultSubset": { "target": "zig-server" }
    }
  },
  "platforms": ["fake-host", "macos", "ios-simulator", "android-emulator", "windows", "linux", "server"]
}
```

`expectedByPlatform` is only for intentional platform-identity differences such as `runtime.capabilities.target` or for a host that rejects an invalid fixture earlier than bridge dispatch. Otherwise fixtures use `expected` and every target must match the fake host.

The harness:

1. Resolves the fixture file.
2. Spawns or attaches to each target platform.
3. Applies `preconditions` via the dev control plane.
4. Sends the request via the standard bridge dispatch path.
5. Compares the response to `expectedByPlatform[target]` when present, otherwise `expected`, stripping non-deterministic fields (`id`, `timestamp`, durations) and any field listed under `expected.ignore`.
6. Fails the fixture if any platform produces a different response than the fake host (which is the reference) without an explicit platform-specific expectation.

### 6.2 Required fixtures

```text
valid-storage-get.json
valid-storage-set.json
valid-storage-list.json
valid-storage-remove.json
invalid-unknown-method.json
invalid-permission-denied.json
invalid-storage-prefix.json
valid-core-step.json
invalid-core-step-bad-json.json
valid-network-request-mocked.json
valid-network-policy-denied.json
valid-dialog-open-mocked.json
valid-dialog-cancelled.json
valid-runtime-capabilities.json
budget-exceeded-bridge-calls.json
runtime-version-incompatible.json
```

### 6.3 Reference contract

The fake host (docs/32) is the reference. Every other platform must match its responses byte-for-byte after stripping the listed non-deterministic fields. Drift between platforms is a bug in whichever platform diverges from the fake host, unless the fake host itself is non-conformant.

## 7. Native platform tests

### iOS

- App launches in simulator.
- WKWebView loads runtime.
- Example launcher visible.
- JS bridge receives request.
- Storage persists across relaunch.
- Core step returns real Zig output.
- Permission denied path works.
- Local simulator build/package smoke runs with `node --test --no-warnings tools/fake-platform-host/test/ios-native-build.test.js` on macOS hosts with Xcode.
- Runtime-load, WK bridge, storage-persistence, and `core.step` launch smoke runs with `NATIVE_AI_IOS_SMOKE_LAUNCH=1 node --test --no-warnings tools/fake-platform-host/test/ios-native-build.test.js` when CoreSimulator and Zig are available.

### macOS

- App launches.
- Runtime loads from bundle.
- Open/save dialogs return results or cancel errors.
- Storage persists.
- Core bridge works.
- Local build and native SwiftPM tests run with `node --test --no-warnings tools/fake-platform-host/test/macos-native-build.test.js` on macOS hosts.
- When Zig is available, the local SwiftPM test builds a temporary macOS `libzig_core.dylib` and verifies native `core.step` returns real Zig actions.
- Debug app launch smoke runs with `NATIVE_AI_MACOS_SMOKE_LAUNCH=1 node --test --no-warnings tools/fake-platform-host/test/macos-native-build.test.js`.

### Android

- App launches in emulator.
- WebView loads runtime from assets.
- JS bridge dispatches messages.
- Storage persists.
- JNI core step works for arm64 and x86_64 debug builds.
- Permission denied path works.
- Local debug APK/JNI/resource build smoke runs with `node --test --no-warnings tools/fake-platform-host/test/android-native-build.test.js` when Gradle and the Android SDK are available.

### Windows

- App launches.
- WebView2 initializes.
- Runtime loads from resources/local folder.
- Bridge dispatch works.
- Storage under LocalAppData works.
- Zig DLL loads.

### Linux

- GTK app launches.
- WebKitGTK loads runtime.
- Bridge dispatch works.
- Storage under XDG data directory works.
- Zig shared library loads.

### Server

- Server starts.
- `/health` returns success.
- `/core/step` matches core contract.
- Invalid request returns structured error.
- Source compile/executable smoke runs with `node --test --no-warnings tools/fake-platform-host/test/server-zig-build.test.js`.
- Bridge fixture contract runs against a fresh Zig server database per fixture with `node --test --no-warnings tools/fake-platform-host/test/server-bridge-contract.test.js`.
- API smoke runs against a local server process with `mdok run tests/server/server-api-smoke.md`.

## 8. End-to-end tests

For each platform shell:

1. Launch app.
2. Open launcher.
3. Open Notes Lite.
4. Create note.
5. Confirm storage.set call.
6. Restart app.
7. Confirm note persists.
8. Open Task Workbench.
9. Create task via core.step.
10. Confirm returned action displayed.
11. Open File Transformer.
12. Trigger dialog open; cancel; verify `dialog_cancelled` handled.
13. Open API Dashboard.
14. Trigger network request with mock endpoint.
15. Open Core Replay Lab.
16. Replay fixture and export.

## 9. Platform smoke execution

The checked-in suite is `tests/platform-smoke/all-example-apps.platform-smoke.json`. It targets `fake-host`, `macos`, `linux`, `windows`, `android-emulator`, and `ios-simulator` with the same per-app flow:

1. `platform.open_webapp`
2. `runtime.wait_for`
3. `runtime.screenshot`
4. `runtime.assert_no_console_errors`
5. `runtime.run_smoke_tests`

Automated baseline:

```sh
node --test --no-warnings tools/fake-platform-host/test/platform-smoke.test.js
```

Manual native execution, when the platform toolchain or device is available:

1. Launch the target host and confirm its control endpoint/token are available.
2. Run the same suite through `platform.run_platform_smoke` with the target `platform` value.
3. Save the run output, screenshots, and any host logs with the release evidence.
4. Treat any fake-host/native drift as a platform bug unless the fake host violates the bridge contract.

Required manual target values:

- `macos`
- `linux`
- `windows`
- `android-emulator`
- `ios-simulator`

## 10. Security tests

Create malicious packages under `tests/security/malicious-packages/`:

- Uses `eval`.
- Uses `fetch`.
- Uses `localStorage`.
- Calls unknown bridge method.
- Calls storage with another app's prefix.
- Adds remote script tag.
- Adds remote CSS import.
- Adds nested iframe.
- Excessive bridge calls.
- Huge package size.
- Huge storage write.

Expected result: rejected at install or denied at runtime.

## 11. Performance tests

Full methodology and targets live in docs/22 §7 (warm-up, sample size, p50/p95 reporting, per-platform context). Summary:

| Metric | Desktop p50 | Desktop p95 | Mobile p50 | Mobile p95 |
|---|---:|---:|---:|---:|
| Runtime launcher load | 400 ms | 1000 ms | 800 ms | 2000 ms |
| Example app cold load | 200 ms | 500 ms | 350 ms | 1000 ms |
| Bridge roundtrip storage.get | 5 ms | 20 ms | 10 ms | 50 ms |
| Core step roundtrip (trivial event) | 5 ms | 20 ms | 12 ms | 50 ms |

Required benchmark scenarios under `tests/performance/`:

- `bridge-latency/` — 500-iteration round-trip after 50-iteration warm-up; report p50/p95 per method.
- `core-step-latency/` — same methodology over `core.step`.
- `app-switch/` — mount/unmount/mount of two example apps; measure time to `runtime.ready`.
- `large-list/` — 1000-row virtual table; measure first paint and scroll jank (frame budget misses).
- `network-timeout/` — confirm `timeoutMs` is respected within ±10%.
- `install-uninstall-loop/` — install + activate + uninstall 50 times; assert no memory or DB growth.

A p95 miss on any platform fails CI for that platform.

## 12. Accessibility tests

- Keyboard navigation in runtime launcher.
- Focus visible on buttons/inputs.
- Dialog focus trap.
- Color contrast light/dark mode.
- Labels for form fields.
- Screen-reader-friendly names for major controls.

## 13. Regression tests

Every bug should become one of:

- core fixture
- bridge fixture
- runtime unit test
- webapp smoke test
- platform smoke test
- malicious package fixture

## 14. CI matrix

```text
Ubuntu:
  Zig core tests
  server tests
  runtime unit tests
  package validation
  Linux shell build/smoke if dependencies installed

macOS:
  Zig core macOS build
  iOS simulator smoke
  macOS app smoke
  runtime tests

Windows:
  Zig core Windows build
  Windows shell build
  WebView2 smoke

Android:
  Android debug build
  emulator smoke
  JNI core tests
```

## 15. Manual test checklist

Before release:

- Open all example apps on every platform.
- Trigger every allowed bridge method at least once.
- Trigger at least 5 permission-denied paths.
- Restart each app and verify persistence.
- Run one invalid package rejection demo.
- Run one core replay demo.
- Export logs from debug console.


## Codex micro-testing layer

The platform must support granular AI-agent-driven tests through the Codex control plugin.

### New test levels

| Level | Scope | Driver |
|---|---|---|
| Runtime unit | Router, permissions, sanitizer, bridge policy | JS unit tests |
| Generated app contract | Package shape, manifest, smoke-tests, bridge method usage | package validator |
| Micro UI | Selector-level DOM interactions inside one generated app | Codex MCP / control plane |
| Bridge contract | Request/response schema, permission denial, logging | Codex MCP / host tests |
| Zig core contract | Event -> action determinism, replay, error handling | Zig tests + Codex MCP |
| Host integration | Native bridge to runtime and Zig | platform-specific tests |
| Cross-platform smoke | Load each example app on every host | Codex MCP orchestration |
| Fault injection | offline network, storage failure, permission denial, timer advance | Codex MCP |

### Minimum Codex-run micro-tests

For every example webapp:

1. Install package through `platform.install_webapp_package`.
2. Open app through `platform.open_webapp`.
3. Wait for runtime idle.
4. Assert the main heading is visible.
5. Execute every declared `smoke-tests.json` step.
6. Capture screenshot.
7. Assert no console error.
8. Assert no denied bridge call unless the test expects a denial.
9. Export event/bridge logs.
10. Replay core events and assert the same final actions.

### Micro-test selectors

Generated apps must include stable selectors for testable elements:

```html
<button data-testid="new-note-button">New Note</button>
<input data-testid="note-title-input">
```

Codex must prefer `data-testid` selectors over CSS class names.

### Repair loop

If a generated app fails a micro-test, Codex should:

1. Read the failure report.
2. Inspect DOM, console, bridge logs, and app source.
3. Patch only the generated app package unless the failure proves a platform/runtime bug.
4. Re-run the failing test.
5. Re-run the app's full smoke suite.
6. Update the failure report with the patch summary.

## v0.3 test-plan additions

### Trust/signing tests

- Valid source package produces valid signature and install report.
- Modified file after signing fails mount.
- Modified permissions after signing fail mount.
- `none-dev` signatures are accepted only on fake-host/dev mode.
- Real native dev targets run signing path, not unsigned direct execution.

### Versioning/rollback tests

- Installing a new version leaves previous version immutable.
- Failed micro-test quarantines new version and keeps previous version active.
- Rollback restores active pointer and smoke-tests the restored app.
- Rollback refuses if dataVersion is incompatible and no snapshot/down-migration exists.

### Migration tests

- Upgrade from dataVersion 1 to 2 runs migration files in order.
- Migration failure restores pre-migration snapshot.
- Codex changes storage shape only with migration and updated tests.

### Capability tests

- Missing required capability prevents mount with clear error.
- Missing optional capability returns `CAPABILITY_UNAVAILABLE` when called.
- Platform capability reports validate against schema on every target.

### Snapshot/replay tests

- Snapshot captures storage, bridge/core logs, capabilities, resource usage, and app version.
- Restored snapshot reproduces visible UI and core outputs on fake-host.
- Cross-platform replay differences are reported, not hidden.

### Resource-budget tests

- Excess bridge calls are blocked.
- Excess storage writes are blocked.
- Large package/file is rejected by validator.
- DOM explosion is detected in dev runtime.

### Accessibility tests

- Unlabeled inputs fail or warn according to install gate.
- Keyboard navigation works for example apps.
- Dialogs preserve focus behavior where dialogs exist.

### Network-policy tests

- Direct `fetch` is rejected by static policy.
- Disallowed origin/method/header is rejected by network bridge.
- Redirects to disallowed origins are rejected.
- Response size and timeout limits are enforced.

### Mutation tests

Add fixtures under `tests/mutation/` for missing manifest fields, forbidden JS APIs, invalid permissions, external scripts, invalid network policy, bad storage prefix, and post-signature tampering.

## 16. Database and persistence tests

Database test requirements:

```text
migration up/down
schema creation
app install transaction
app rollback
storage get/set/list/remove
permission versioning
bridge log insert
core event/action insert
runtime snapshot insert
micro-test run insert
backup export/import round trip
database corruption handling
```

Required database test fixtures live under `tests/db/`.

The first executable database test should apply all SQLite migrations to an in-memory database and verify required tables. The second should install one generated app package transactionally and verify rows in `apps`, `app_versions`, `app_files`, `app_permissions`, `app_install_reports`, and `app_installations`.

Postgres production schema must be statically checked in CI at minimum and applied to a Postgres container when available.
