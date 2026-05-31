# Implementation Status

This is the single source of truth for what is built vs planned. Update this file in the same commit as any change that crosses a status line.

Legend:

- **spec-only** — doc exists; no code/skeleton.
- **schema** — JSON schema present; no implementation.
- **skeleton** — README or placeholder directory only.
- **fixtures** — sample data / fixtures present.
- **partial** — implementation under way but not feature-complete.
- **complete** — implementation passes the contract tests for its surface.

Status snapshot: **2026-05-31**.

## Documents

| Path | Status | Owner |
|---|---|---|
| `docs/00_PRD.md` | spec-only (v0.4) | platform |
| `docs/01_ARCHITECTURE.md` | spec-only (v0.4) | platform |
| `docs/02_PROJECT_STRUCTURE.md` | spec-only | platform |
| `docs/03_RUNTIME_API_SPEC.md` | spec-only (v0.4) | runtime |
| `docs/04_WEBAPP_PACKAGE_SPEC.md` | spec-only (v0.4) | runtime |
| `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md` | spec-only (v0.4) | native |
| `docs/06_ZIG_CORE_SPEC.md` | spec-only | zig |
| `docs/07_SECURITY_MODEL.md` | spec-only (v0.4) | platform |
| `docs/08_TEST_PLAN.md` | spec-only (v0.4) | qa |
| `docs/09_CODEX_IMPLEMENTATION_PLAN.md` | spec-only | codex |
| `docs/10_ACCEPTANCE_CHECKLIST.md` | spec-only | qa |
| `docs/11_AI_GENERATION_PROMPTS.md` | spec-only | codex |
| `docs/12_RELEASE_AND_CI.md` | spec-only | platform |
| `docs/13_EXAMPLE_APP_COVERAGE.md` | spec-only (v0.4) | qa |
| `docs/14_CODEX_CONTROL_PLUGIN.md` | spec-only (v0.4) | codex |
| `docs/15_MICRO_TESTING_PROTOCOL.md` | spec-only (v0.4) | qa |
| `docs/16_CODEX_PLUGIN_IMPLEMENTATION_PLAN.md` | spec-only | codex |
| `docs/17_APP_SIGNING_AND_TRUST.md` | spec-only (v0.4) | platform |
| `docs/18_APP_VERSIONING_AND_ROLLBACK.md` | spec-only | platform |
| `docs/19_DATA_MIGRATIONS.md` | spec-only (v0.4) | platform |
| `docs/20_RUNTIME_CAPABILITIES.md` | spec-only | runtime |
| `docs/21_SNAPSHOT_AND_REPLAY_FORMAT.md` | spec-only | runtime |
| `docs/22_RESOURCE_BUDGETS.md` | spec-only (v0.4) | runtime |
| `docs/23_ACCESSIBILITY_CONTRACT.md` | spec-only (v0.4) | runtime |
| `docs/24_NETWORK_POLICY.md` | spec-only | runtime |
| `docs/25_CODEX_REPAIR_LOOP.md` | spec-only | codex |
| `docs/26_PLATFORM_CAPABILITY_MATRIX.md` | spec-only | native |
| `docs/27_DATABASE_SCHEMA.md` | spec-only | platform |
| `docs/28_STORAGE_AND_MIGRATIONS.md` | spec-only | platform |
| `docs/29_BACKUP_EXPORT_IMPORT.md` | spec-only | platform |
| `docs/30_DATABASE_TEST_PLAN.md` | spec-only | qa |
| `docs/31_V0_4_INTEGRATION_MAP.md` | spec-only | platform |
| `docs/32_FAKE_HOST_SPEC.md` | spec-only (new in v0.4 revision) | platform |

## Schemas

| Path | Status |
|---|---|
| `schemas/manifest.schema.json` | schema |
| `schemas/app-package.schema.json` | schema |
| `schemas/bridge-request.schema.json` | schema |
| `schemas/bridge-response.schema.json` | schema |
| `schemas/core-step.schema.json` | schema |
| `schemas/app-signature.schema.json` | schema |
| `schemas/app-migration.schema.json` | schema |
| `schemas/runtime-capabilities.schema.json` | schema |
| `schemas/runtime-snapshot.schema.json` | schema |
| `schemas/network-policy.schema.json` | schema |
| `schemas/resource-budget.schema.json` | schema |
| `schemas/install-report.schema.json` | schema |
| `schemas/app-version-record.schema.json` | schema |
| `schemas/accessibility-report.schema.json` | schema |
| `schemas/db-app-records.schema.json` | schema |
| `schemas/db-runtime-records.schema.json` | schema |
| `schemas/db-test-records.schema.json` | schema |
| `schemas/backup-export.schema.json` | schema |
| `schemas/dev-control-command.schema.json` | schema |
| `schemas/dev-control-response.schema.json` | schema |
| `schemas/micro-test.schema.json` | schema |
| `schemas/mutation-fixture.schema.json` | schema |
| `schemas/bridge-contract-fixture.schema.json` | schema |

## Code/runtime directories

| Path | Status | Notes |
|---|---|---|
| `runtime-web/` | partial | Launcher HTML/CSS/JS exists; fake host serves it and proxies sandbox `AppRuntime.call` messages to `/bridge` or WebKit/Android/WebView2 native bridge with `AppRuntime.on` events, optional host-provided bundled app index/content ratings, per-mount nonce/port binding, generated-app iframe CSP/referrer/no-feature-delegation attributes with app-runtime package-resource allowances, permission, storage-prefix, network-policy, rate-budget preflight, srcdoc package-resource URL rewriting without injected `<base>`, PRD-aligned `sandbox="allow-scripts"` mounting for all generated app frames including WebKit native, `app.budget_warning` and unauthorized-channel `app.error` delivery through the assigned port, browser-only dev mock dispatch with Zig-aligned core.step demo actions, development-only `window.__APP_RUNTIME_DEVTOOLS__` snapshot/query/bridge/console/storage/core/reset hooks, and dev DOM/timer budget guards |
| `zig-core/` | partial | Zig 0.15.2 static/dynamic library package with C FFI header, deterministic JSON core.step behavior, unit tests, and focused direct Zig build/test smoke coverage for native static/shared library artifacts |
| `server/` | partial | Minimal Zig HTTP server with `/health`, `/core/step`, `/bridge` for core/capabilities/storage/mock-dialogs/notification/mock-network/app.log, `/webapps/validate`, `/webapps/install`, `/packages/*`, `/apps/{appId}/rollback`, `/webapps/examples`, `/control/command`, and token-gated safe `/db/*` inspection endpoints; uses Zig core FFI, writes a per-launch control-token file in dev mode, initializes the v0.4 logical SQLite schema, signs inline packages with Ed25519, gates installs and bridge dispatch with runtimeVersion compatibility checks, gates installs with static bundled smoke and accessibility checks, enforces package hard file/migration-count caps, rejects source packages with platform-generated trust artifacts, enforces plain `app.js` script, `styles.css` link, appId-free bridge params, and inline-style/CSP policies, enforces active-install bridge permissions with contract details, enforces active manifest `resourceBudget` limits for bridge calls/network requests/log lines/storage bytes with repeated-violation quarantine/restore, enforces `networkPolicy` before mock-backed network responses and validates mock response timeout/size/redirect policy, validates bundled App Store content-rating metadata, serves file dialogs from `dialog_mocks`, installs packages transactionally with packaged migration chains, supports data-version-aware package rollback with snapshot data restore plus uninstall/update-approval/quarantine lifecycle controls, creates/restores runtime snapshots, runs migration dry-run/apply controls, exposes server-side runtime open/confirmation-gated reset/storage/static-snapshot/static-UI/accessibility/log/resource/test-runner/repair-loop control tools plus bridge-call/core-step/core-replay/assertion/timer/notification/snapshot-compare/fault-injection controls, disables dev/control endpoints in production mode, persists bridge/core/test logs, records backup/debug bundle exports and transactional backup imports, audits accepted/rejected control requests, temporarily bans repeated control auth failures, and has focused compile/executable smoke, server bridge fixture contract coverage, plus mdok `/health` and `/core/step` API smoke coverage |
| `tools/fake-platform-host/` | partial | Node fake host with SQLite migrations, per-launch control-token file support, audited control auth failures with temporary ban, session/command/package/app/db control HTTP surface, package validation/install with smoke-test and accessibility gates, appId-free bridge-param source policy, hard file/migration-count caps, runtime-compatibility, update-approval gating, and confirmation-gated destructive uninstall/reset controls, Ed25519 signing with configured persistent platform key-file support and public-key health metadata, verified mount gate, version rollback/quarantine, snapshots, migrations, backup export/import, bridge dispatch with resource-budget checks plus automatic repeated-violation quarantine/restore, static runtime controls, `app.log`-backed console-log inspection/assertion, persisted-bridge `notification.toast` capture, devtools-enabled runtime shell serving for fake-host dev/test sessions, static plus optional Chrome/CDP browser-backed smoke runner, static micro-test runner, repair-loop patch/retest support, and focused tests |
| `tools/codex-platform-mcp/` | partial | Dependency-free MCP stdio JSON-RPC server exposes per-tool JSON Schema input contracts, validates arguments and destructive-call confirmations before forwarding declared tools to fake-host/dev control plane, reads per-launch control tokens from the documented token file unless explicitly overridden for tests, and has node:test coverage |
| `tools/package-release.mjs` | partial | Dependency-free release artifact packager writes deterministic `runtime-web.zip`, `example-webapps.zip`, target-output directories, optional docs/05 Zig core target libraries via `--build-zig-core`, optional host-native Zig server executable output via `--build-server`, optional macOS host `.app` output with bundled runtime/examples/SQLite migrations/`libzig_core.dylib` via `--build-native-macos`, optional Linux host app output with bundled runtime/examples/SQLite migrations/`libzig_core.so` plus GTK/WebKitGTK/Meson preflight via `--build-native-linux`, optional Windows host app output with bundled runtime/examples/SQLite migrations/`zig_core.dll` plus WebView2 SDK preflight via `--build-native-windows`, and `release-manifest.json` with hashes |
| `native/ios/` | partial | SwiftPM/UIKit/WKWebView scaffold with app-runtime custom-scheme resource loading, generated-app app-runtime index responses with a port-only `AppRuntime` bootstrap and app-runtime CSP allowances, WKScriptMessageHandlerWithReply, runtime bridge envelope handling with asynchronous replies plus host-side appId request-body rejection and strict envelope/request field validation, build-mode-derived `runtime.capabilities.devMode`, SQLite storage through a `PlatformDatabase` opener that applies bundled or checked-in SQLite migrations and runs `PRAGMA integrity_check`, host-served bundled app index with App Store content ratings, bundled-only bridge gating, and a debug/simulator age gate, native open/save file dialogs, manifest-gated network requests, native permission checks, native storage `maxStorageBytes` enforcement, native bridge/network rate-budget enforcement, native `notification.toast` message/level validation, native `app.log` level/message validation plus manifest log-rate budget enforcement, DB-backed `runtime_sessions`, `bridge_calls`, `core_events`, and `core_actions` logging for native bridge dispatch, `core.step` through a linked-or-dlopen-backed Zig C shim when Zig core symbols or `libzig_core.dylib` are available, debug-only simulator build/package/runtime-load/bridge/storage-persistence/core-step launch smoke coverage with persisted bridge/core log rows verification when Xcode, CoreSimulator, and Zig are available, and structured unsupported responses for unfinished platform services |
| `native/macos/` | partial | SwiftPM AppKit/WKWebView host scaffold with explicit AppKit entry point, app-runtime custom-scheme resource loading, generated-app app-runtime index responses with a port-only `AppRuntime` bootstrap and app-runtime CSP allowances, runtime bridge envelope handling with host-side appId request-body rejection and strict envelope/request field validation, build-mode-derived `runtime.capabilities.devMode`, WebView content-process termination recovery with failed `runtime_sessions` recording and reload banner, SQLite storage through a `PlatformDatabase` opener that applies checked-in SQLite migrations and runs `PRAGMA integrity_check`, storage bridge `storage_error` responses for SQLite open/prepare/step failures, SQLite app-version registry rollback that restores the previous active install while preserving generated app storage, open/save dialogs, manifest-gated network requests, native storage `maxStorageBytes` enforcement, native bridge/network rate-budget enforcement with repeated-violation quarantine/restore for active installs, native `notification.toast` message/level validation, native `app.log` level/message validation plus manifest log-rate budget enforcement, production guard rejection and audit for dev-only startup flags outside DEBUG builds, DB-backed `runtime_sessions`, `bridge_calls`, `core_events`, and `core_actions` logging for native bridge dispatch, structured bridge responses, nonblocking WebView `core.step` dispatch with 2000 ms timeout errors, `core.step` through a dlopen-backed Zig C shim when `libzig_core.dylib` is available, debug-only loopback control plane startup with per-launch 0600 token file, token-gated health/session lifecycle/snapshot/events/capabilities/resource-usage/accessibility routes, lifecycle target list/launch/reload/open-webapp controls backed by runtime session rows with active-install signature/content verification before open, app registry list/version/install-report/rollback/quarantine/uninstall/update-approval controls with packaged data-version migration approval, dev-control package validate/Keychain-backed Ed25519-sign/install that persists app versions/files/permissions/install reports and writes failed install reports on storage transaction failure, static runtime screenshot/query/click/type/set-value/key/drag controls, visible/text assertions, static wait, and no-op timer controls over generated app HTML, runtime storage get/set plus confirmation-gated reset controls with bridge-call recording plus bridge-call/log inspection, notification capture, DB-backed network/dialog mock controls, DB-backed one-shot bridge fault injection, storage assertions, core snapshot/action assertions, fresh-core replay, normalized stable-hash snapshot comparison, declarative migration dry-run/apply with pre-migration snapshots and persisted `migration_runs`, static bundled smoke-test execution, checked-in static micro-test execution, and checked-in platform-smoke execution with persisted `test_runs`, permission-checked `runtime.call_bridge` and `runtime.core_step` controls with bridge/core DB logging, persisted runtime snapshot create/read/restore controls over `runtime_snapshots`, `platform.health` with signing public-key metadata, `runtime.capabilities`, DB-backed `runtime.resource_usage`, and static accessibility snapshot/audit/assertion command routing, safe `db.snapshot` plus fixed `db.query_app_storage`/`db.query_app_versions`/`db.query_bridge_calls`/`db.query_core_events`/`db.query_test_runs` inspection over SQLite tables, `db.export_backup`/`db.import_backup` plus `db.export_debug_bundle` records in `backup_exports`, and audited accepted/rejected requests, plus SwiftPM tests covering runtime resource loading, bridge dispatch, control-plane auth/audit/session/lifecycle/package-install/app-registry/db/accessibility/storage/log/bridge/core/snapshot/migration/backup/smoke-test/micro-test/platform-smoke routes, dialog open/save/cancel responses, storage persistence, rollback, production guard dev-flag rejection/audit, dev-control Keychain-backed Ed25519 package signing, tampered installed-package open rejection, Zig-backed `core.step` with a temporary dylib when Zig is available, and debug app launch smoke |
| `native/android/` | partial | Kotlin Android scaffold with generated shared runtime/example assets and checked-in SQLite migrations, WebViewAssetLoader, WebViewCompat runtime envelope handling with origin/frame checks, asynchronous replies, host-side appId request-body rejection, and strict envelope/request field validation, BuildConfig-derived `runtime.capabilities.devMode`, SQLite-backed storage through a `PlatformDatabase` opener that applies bundled SQLite migrations and runs `PRAGMA integrity_check`, native open/save file dialogs, manifest-derived native context, manifest-gated network requests, native permission checks, native storage `maxStorageBytes` enforcement, native bridge/network rate-budget enforcement, native `notification.toast` message/level validation, native `app.log` level/message validation plus manifest log-rate budget enforcement, DB-backed `runtime_sessions`, `bridge_calls`, `core_events`, and `core_actions` logging for native bridge dispatch, Gradle-packaged Android ABI `libzig_core.so` artifacts plus `core.step` through a JNI wrapper that loads them, structured unsupported responses for unfinished platform services, debug APK/JNI/resource/Zig-core packaging build-smoke coverage when Gradle, Zig, and the Android SDK are available, and optional AVD smoke coverage for runtime asset load, bridge-backed storage persistence, persisted bridge/core log rows, and JNI-backed `core.step` |
| `native/windows/` | partial | C++/WinRT/WebView2 scaffold with virtual-host loading, WebView2 1.0.2592+ runtime gating, envelope-only runtime bridge handling with host-side appId request-body rejection plus strict required id/method/params and top-level field validation, WebMessageReceived origin checks, build-mode-derived `runtime.capabilities.devMode`, SQLite-backed storage through a `PlatformDatabase` opener that prefers packaged `resources/db/sqlite` migrations, falls back to checked-in SQLite migrations, and runs `PRAGMA integrity_check`, structured storage errors for SQLite prepare/step failures, native open/save file dialogs, manifest-gated network requests with request `timeoutMs` validation, effective timeout clamping, and structured WinHTTP timeout errors, native permission checks, native storage `maxStorageBytes` enforcement, native bridge/network rate-budget enforcement, native `notification.toast` message/level validation, native `app.log` level/message validation plus manifest log-rate budget enforcement, production guard rejection/audit for dev-only startup flags outside debug builds, DB-backed `runtime_sessions`, `bridge_calls`, `core_events`, and `core_actions` logging for native bridge dispatch, `core.step` through a LoadLibrary-backed Zig DLL loader that prefers `NATIVE_AI_ZIG_CORE_DLL`, then the executable-adjacent packaged `zig_core.dll`, then repo-local dev fallbacks, Windows release packaging that stages runtime/examples/SQLite resources with `zig_core.dll` behind WebView2 SDK/x64 preflights, structured unsupported responses for unfinished platform services, and optional CMake smoke coverage for release-build production guard rejection/audit, runtime JS/readiness after virtual-host navigation, generated-app `AppRuntime.call("storage.get")` WebView2 bridge dispatch against seeded storage, bridge-backed storage persistence, persisted bridge/core log rows, fixed bridge methods (`storage.list`, `storage.remove`, `notification.toast` message/level validation, `app.log`, `runtime.capabilities`, manifest-denied `network.request`), and Zig DLL-backed `core.step` from the packaged executable directory when Windows/WebView2 build dependencies are available |
| `native/linux/` | partial | C/GTK4/WebKitGTK scaffold with secure/CORS-enabled custom scheme loading that prefers packaged `resources/runtime` and `resources/webapps/examples` beside the executable, falls back to checked-in `runtime-web/` and `webapps/examples/` for dev runs, and loads manifest permissions/budgets/network policy from the same packaged-or-dev app source, generated-app app-runtime index responses are served with a port-only `AppRuntime` bootstrap and app-runtime CSP allowances so bridge calls route through the parent runtime MessageChannel instead of a direct native bridge, reply-capable runtime bridge envelope handling with host-side appId request-body rejection plus strict envelope and bridge request id/method/params and top-level field validation, build-mode-derived `runtime.capabilities.devMode`, SQLite-backed storage through a `PlatformDatabase` opener that applies packaged `resources/db/sqlite` or checked-in SQLite migrations and runs `PRAGMA integrity_check`, structured storage errors for SQLite prepare/step failures, native open/save file dialogs, manifest-gated network requests, native permission checks, native storage `maxStorageBytes` enforcement, native bridge/network rate-budget enforcement, native `notification.toast` message/level validation, native `app.log` level/message validation plus manifest log-rate budget enforcement, production guard rejection/audit for dev-only startup flags outside debug builds, DB-backed `runtime_sessions`, `bridge_calls`, `core_events`, and `core_actions` logging for native bridge dispatch, `core.step` through a dlopen-backed Zig C ABI loader that honors `NATIVE_AI_ZIG_CORE_SO`, then bundled `libzig_core.so` beside `native-ai-webapp-host`, then dev/install fallbacks, structured unsupported responses for unfinished platform services, Docker-backed Meson/Xvfb smoke coverage for runtime load, generated-app `AppRuntime.call` WebKitGTK bridge dispatch, bridge-backed storage persistence, persisted bridge/core log rows, fixed bridge methods (`storage.list`, `storage.remove`, `notification.toast` message/level validation, `app.log`, `runtime.capabilities`, manifest-denied `network.request`), Zig shared-library `core.step`, release-build production guard rejection with persisted SQLite audit evidence, and packaged artifact launch coverage from outside the repo root without a Zig-core environment override |
| `codex-plugin/platform-control/` | partial | `plugin.json`, `.mcp.json`, repo-local marketplace entry, and workflow skills for platform micro-tests, generated webapp repair, and core replay debugging are present; local MCP path resolves to the repo server |
| `devtools/control-plane/` | partial | `openapi.json` + README document token-gated command/session routes plus safe DB snapshot/query/backup/import/debug-bundle endpoints, with static OpenAPI guard coverage |

## Example apps

| Path | Status |
|---|---|
| `webapps/examples/notes-lite/` | fixtures (manifest/HTML/CSS/JS + smoke-tests) |
| `webapps/examples/task-workbench/` | fixtures |
| `webapps/examples/file-transformer/` | fixtures |
| `webapps/examples/api-dashboard/` | fixtures |
| `webapps/examples/core-replay-lab/` | fixtures |

## Database migrations

| Path | Status |
|---|---|
| `db/sqlite/001_initial.sql` | schema |
| `db/sqlite/002_runtime_debug.sql` | schema |
| `db/sqlite/003_codex_control.sql` | schema |
| `db/sqlite/004_migrations_and_snapshots.sql` | schema |
| `db/postgres/001_initial.sql` | schema |
| `db/postgres/002_runtime_debug.sql` | schema |
| `db/postgres/003_codex_control.sql` | schema |
| `db/postgres/004_migrations_and_snapshots.sql` | schema |

## Tests

| Path | Status |
|---|---|
| `tests/fixtures/bridge/` | partial (31 fixtures are exercised by fake-host bridge tests and Zig server bridge contract tests: the expanded docs/08 required fixtures now cover every fixed bridge method, network policy edge cases, and focused core.step parity for `CreateTask`, `NetworkSnapshotReceived`, and unknown events) |
| `tests/fixtures/capabilities/` | partial (schema-shaped runtime capability fixtures for fake-host, server, macOS, iOS simulator, Android, Windows, and Linux are exercised by runtime capability contract tests) |
| `tests/fixtures/db/` | partial (schema-shaped app/install/runtime/test/control fixture records include control audit fields plus network, dialog, and fault-injection effect mocks) |
| `tests/fixtures/snapshots/` | partial (checked-in runtime snapshot fixture is schema-validated and exercised through fake-host `runtime.compare_snapshot` controls) |
| `tests/golden/` | partial (6 checked-in golden fixtures are schema-validated; the minimal-counter package materializes into a package-validator-accepted v0.4 source package, installs on fake-host, runs its bundled smoke test, persists the smoke run, and the five micro-test style flows covering storage forms, network policy, file dialogs/core, core.step replay, and large-table budgets execute through the fake-host micro-test runner) |
| `tests/micro/` | partial (5 micro-tests, one per bundled example app, execute through the fake-host micro-test runner and persist `test_runs`) |
| `tests/mutation/` | partial (42 mutations now exercised by fake-host validator/runtime tests, including direct network/storage/native-bridge APIs, sendBeacon, Cookie Store API use, smoke selector stability and bundled smoke command-surface rejection, resource-hint, external HTML resource, and non-package CSS URL rejection, appId bridge-param rejection, service workers, sandbox escape APIs, inline-style CSP drift, app script/stylesheet tag drift, platform-generated artifact rejection, invalid manifests/capabilities/resource budgets, tampering, and runtime-denied paths) |
| `tests/accessibility/` | partial (checked-in accessibility microtests execute against fake-host controls; every example app passes the fake-host static accessibility audit) |
| `tests/performance/` | partial (fake-host harness reports p50/p95 for launcher initial load, app open, app switch, storage, and core bridge round-trips, plus bridge-throughput, open-all-examples memory, large-list windowing, network-timeout, and install/uninstall lifecycle cleanup scenarios) |
| `tests/security/malicious-packages/` | partial (12 fixtures exercised by fake-host security tests; static rejection plus runtime-denied storage-prefix and budget paths) |
| `tests/db/` | partial (9 checked-in dbtest fixtures are exercised by fake-host DB contract tests, including schema, install, storage, runtime logging, rollback, migration, backup, and corruption paths) |
| `tests/platform-smoke/` | partial (cross-platform suite is checked in and exercised by the fake-host runner with persisted `test_runs`; native target execution remains environment-dependent) |
| `tests/server/` | fixtures (mdok smoke verifies the Zig server starts and serves `/health` plus `/core/step`) |

## CI

Initial remote CI is wired in `.github/workflows/ci.yml` around `tools/check-repo.mjs`, Zig core/server tests, fake-host tests, target-enforced fake-host performance smoke with report artifact upload, Codex MCP contract tests, static release artifact packaging/upload, macOS-built Zig core release artifact packaging/upload, Linux-built server release artifact packaging/upload, macOS native host artifact packaging/upload, Linux native host artifact packaging/upload, Windows native host artifact packaging/upload, plus Docker-backed Linux GTK/WebKitGTK, macOS WKWebView, iOS simulator, Android emulator, and Windows WebView2 native launch smoke jobs. `docs/12_RELEASE_AND_CI.md` describes the full matrix. First CI gates:

1. **JSON validate** — every JSON in the repo parses; every fixture validates against its schema.
2. **SQLite migrate** — `db/sqlite/*.sql` applies cleanly to an in-memory SQLite, required tables present.
3. **Postgres lint** — `db/postgres/*.sql` is checked for SQLite/Postgres table and logical-column parity, JSONB use, app-storage primary key shape, and optional live apply when `POSTGRES_TEST_URL` is set.
4. **Canonical example packages** — `webapps/examples/` is the only generated app package source.
5. **Spec lint** — section numbering contiguous; no `addJavascriptInterface` in native source; no `networkAllowlist` in manifests.
6. **Zig tests** — install Zig 0.15.2 and SQLite headers, then run `zig build test` in `zig-core/` and `server/`.
7. **Performance smoke** — run the fake-host latency harness with reduced CI samples, fail p50/p95 target misses, and upload the `performance_runs/` report artifact.
8. **Static release artifact package** — write deterministic static release artifacts (`runtime-web.zip`, `example-webapps.zip`, `release-manifest.json`) and upload the artifact tree for downstream target jobs.
9. **Zig core release artifact package** — build and upload docs/05 Zig core target libraries from the macOS release-artifact job.
10. **Server release artifact package** — build the host-native Zig server executable on `ubuntu-24.04`, record its hash in `release-manifest.json`, and upload the server artifact tree.
11. **macOS native release artifact package** — build the release SwiftPM macOS host into a `.app` bundle, include runtime/example/SQLite resources plus `libzig_core.dylib`, record file hashes in `release-manifest.json`, and upload the native artifact tree.
12. **Linux native release artifact package** — install GTK/WebKitGTK/JSON-GLib/SQLite/libsoup/Meson/Ninja build dependencies on `ubuntu-24.04`, build the release C host, stage runtime/example/SQLite resources plus `libzig_core.so`, record file hashes in `release-manifest.json`, and upload the native artifact tree.
13. **Linux native smoke** — build `native/linux/Dockerfile`, run `tools/run-linux-native-docker.mjs`, build the Linux host inside the container, launch it under Xvfb/DBus, and verify runtime load, generated-app WebKitGTK bridge dispatch, bridge-backed storage persistence, persisted bridge/core log rows, fixed bridge methods, `libzig_core.so` backed `core.step`, release-build production guard rejection/audit for dev-only startup flags, and packaged artifact launch from outside the repo root without a Zig-core environment override.
14. **macOS native smoke** — build and test the SwiftPM AppKit host on `macos-latest`, then run the debug launch smoke with Zig core dylib coverage when available.
15. **iOS simulator smoke** — build the SwiftPM UIKit host for the iOS simulator, package runtime/example/SQLite resources, launch in a simulator, and verify runtime load, storage persistence, and Zig-backed `core.step`.
16. **Android emulator smoke** — assemble the debug APK with runtime/example assets plus packaged Zig/JNI libraries, launch it on an emulator, and verify runtime load, storage persistence, and JNI-backed `core.step`.
17. **Windows native release package** — download the pinned WebView2 SDK package on `windows-2022`, build the C++/WinRT host with CMake, stage runtime/example/SQLite resources plus `zig_core.dll`, record file hashes in `release-manifest.json`, and upload the native artifact tree.
18. **Windows native smoke** — download the pinned WebView2 SDK package on `windows-2022`, build the C++/WinRT host with CMake, launch it, and verify WebView2 version/runtime JS readiness, generated-app WebView2 bridge dispatch against seeded storage, bridge-backed storage persistence, persisted bridge/core log rows, fixed bridge methods, and executable-adjacent `zig_core.dll` backed `core.step` without a launch-time DLL override.

## How to update this file

1. Touch a file. 2. If the file's status changed (skeleton → partial, partial → complete, etc.), edit the row in the same commit. 3. If a new file/directory was added that has a status row's worth of meaning, add a row. 4. If status hasn't changed, leave the file alone — `git blame` is enough.
