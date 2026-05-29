# Implementation Status

This is the single source of truth for what is built vs planned. Update this file in the same commit as any change that crosses a status line.

Legend:

- **spec-only** — doc exists; no code/skeleton.
- **schema** — JSON schema present; no implementation.
- **skeleton** — README or placeholder directory only.
- **fixtures** — sample data / fixtures present.
- **partial** — implementation under way but not feature-complete.
- **complete** — implementation passes the contract tests for its surface.

Status snapshot: **2026-05-29**.

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
| `runtime-web/` | partial | Launcher HTML/CSS/JS exists; fake host serves it and proxies sandbox `AppRuntime.call` messages to `/bridge` or WebKit/Android/WebView2 native bridge with `AppRuntime.on` events, optional host-provided bundled app index/content ratings, per-mount nonce/port binding, permission, storage-prefix, network-policy, rate-budget preflight, `app.budget_warning` delivery for runtime-counted budgets, browser-only dev mock dispatch, and dev DOM/timer budget guards |
| `zig-core/` | partial | Zig 0.15.2 static/dynamic library package with C FFI header, deterministic JSON core.step behavior, unit tests, and focused direct Zig build/test smoke coverage for native static/shared library artifacts |
| `server/` | partial | Minimal Zig HTTP server with `/health`, `/core/step`, `/bridge` for core/capabilities/storage/mock-dialogs/notification/mock-network/app.log, `/webapps/validate`, `/webapps/install`, `/packages/*`, `/apps/{appId}/rollback`, `/webapps/examples`, `/control/command`, and token-gated safe `/db/*` inspection endpoints; uses Zig core FFI, writes a per-launch control-token file in dev mode, initializes the v0.4 logical SQLite schema, signs inline packages with Ed25519, gates installs and bridge dispatch with runtimeVersion compatibility checks, gates installs with static bundled smoke and accessibility checks, enforces package hard file/migration-count caps, enforces active-install bridge permissions with contract details, enforces active manifest `resourceBudget` limits for bridge calls/network requests/log lines/storage bytes with repeated-violation quarantine/restore, enforces `networkPolicy` before mock-backed network responses and validates mock response timeout/size/redirect policy, validates bundled App Store content-rating metadata, serves file dialogs from `dialog_mocks`, installs packages transactionally with packaged migration chains, supports data-version-aware package rollback with snapshot data restore plus uninstall/update-approval/quarantine lifecycle controls, creates/restores runtime snapshots, runs migration dry-run/apply controls, exposes server-side runtime open/reset/storage/static-snapshot/static-UI/accessibility/log/resource/test-runner/repair-loop control tools plus bridge-call/core-step/core-replay/assertion/timer/notification/snapshot-compare/fault-injection controls, disables dev/control endpoints in production mode, persists bridge/core/test logs, records backup/debug bundle exports and transactional backup imports, audits accepted/rejected control requests, temporarily bans repeated control auth failures, and has focused compile/executable smoke, server bridge fixture contract coverage, plus mdok `/health` and `/core/step` API smoke coverage |
| `tools/fake-platform-host/` | partial | Node fake host with SQLite migrations, per-launch control-token file support, audited control auth failures with temporary ban, session/command/package/app/db control HTTP surface, package validation/install with smoke-test and accessibility gates, hard file/migration-count caps, runtime-compatibility, and update-approval gating, Ed25519 signing, verified mount gate, version rollback/quarantine, snapshots, migrations, backup export/import, bridge dispatch with resource-budget checks plus automatic repeated-violation quarantine/restore, static runtime controls, static plus optional Chrome/CDP browser-backed smoke runner, static micro-test runner, repair-loop patch/retest support, and focused tests |
| `tools/codex-platform-mcp/` | partial | Dependency-free MCP stdio JSON-RPC server forwards declared tools to fake-host/dev control plane, reads per-launch control tokens from the documented token file unless explicitly overridden for tests, and has node:test coverage |
| `native/ios/` | partial | SwiftPM/UIKit/WKWebView scaffold with app-runtime custom-scheme resource loading, WKScriptMessageHandlerWithReply, runtime bridge envelope handling with asynchronous replies, SQLite storage through a `PlatformDatabase` opener that applies bundled or checked-in SQLite migrations and runs `PRAGMA integrity_check`, host-served bundled app index with App Store content ratings, bundled-only bridge gating, and a debug/simulator age gate, native open/save file dialogs, manifest-gated network requests, native permission checks, `core.step` through a linked-or-dlopen-backed Zig C shim when Zig core symbols or `libzig_core.dylib` are available, debug-only simulator build/package/runtime-load/bridge/storage-persistence/core-step launch smoke coverage when Xcode, CoreSimulator, and Zig are available, and structured unsupported responses for unfinished platform services |
| `native/macos/` | partial | SwiftPM AppKit/WKWebView host scaffold with explicit AppKit entry point, app-runtime custom-scheme resource loading, runtime bridge envelope handling, SQLite storage through a `PlatformDatabase` opener that applies checked-in SQLite migrations and runs `PRAGMA integrity_check`, SQLite app-version registry rollback that restores the previous active install while preserving generated app storage, open/save dialogs, manifest-gated network requests, toast logging, structured bridge responses, `core.step` through a dlopen-backed Zig C shim when `libzig_core.dylib` is available, debug-only loopback control plane startup with per-launch 0600 token file, token-gated health/session lifecycle/snapshot/events/capabilities/resource-usage/accessibility routes, lifecycle target list/launch/reload/open-webapp controls backed by runtime session rows, app registry list/version/install-report/rollback controls, static runtime screenshot/query/click/type/set-value/key/drag controls, visible/text assertions, static wait, and no-op timer controls over generated app HTML, runtime storage get/set/reset controls with bridge-call recording plus bridge-call/log inspection, notification capture, DB-backed network/dialog mock controls, DB-backed one-shot bridge fault injection, storage assertions, core snapshot/action assertions, fresh-core replay, normalized stable-hash snapshot comparison, and static bundled smoke-test execution with persisted `test_runs`, permission-checked `runtime.call_bridge` and `runtime.core_step` controls with bridge/core DB logging, persisted runtime snapshot create/read/restore controls over `runtime_snapshots`, `platform.health`, `runtime.capabilities`, DB-backed `runtime.resource_usage`, and static accessibility snapshot/audit/assertion command routing, safe `db.snapshot` plus fixed `db.query_app_storage`/`db.query_app_versions`/`db.query_bridge_calls`/`db.query_core_events`/`db.query_test_runs` inspection over SQLite tables, `db.export_debug_bundle` export records in `backup_exports`, and audited accepted/rejected requests, plus SwiftPM tests covering runtime resource loading, bridge dispatch, control-plane auth/audit/session/lifecycle/app-registry/db/accessibility/storage/log/bridge/core/snapshot/smoke-test routes, dialog open/save/cancel responses, storage persistence, rollback, Zig-backed `core.step` with a temporary dylib when Zig is available, and debug app launch smoke |
| `native/android/` | partial | Kotlin Android scaffold with generated shared runtime/example assets and checked-in SQLite migrations, WebViewAssetLoader, WebViewCompat runtime envelope handling with origin/frame checks and asynchronous replies, SQLite-backed storage through a `PlatformDatabase` opener that applies bundled SQLite migrations and runs `PRAGMA integrity_check`, native open/save file dialogs, manifest-derived native context, manifest-gated network requests, native permission checks, Gradle-packaged Android ABI `libzig_core.so` artifacts plus `core.step` through a JNI wrapper that loads them, structured unsupported responses for unfinished platform services, debug APK/JNI/resource/Zig-core packaging build-smoke coverage when Gradle, Zig, and the Android SDK are available, and optional AVD smoke coverage for runtime asset load, bridge-backed storage persistence, and JNI-backed `core.step` |
| `native/windows/` | partial | C++/WinRT/WebView2 scaffold with virtual-host loading, runtime bridge envelope handling, WebMessageReceived origin checks, SQLite-backed storage through a `PlatformDatabase` opener that applies checked-in SQLite migrations and runs `PRAGMA integrity_check`, native open/save file dialogs, manifest-gated network requests, native permission checks, `core.step` through a LoadLibrary-backed Zig DLL loader when `zig_core.dll` is available, structured unsupported responses for unfinished platform services, and optional CMake smoke coverage for runtime load, bridge-backed storage persistence, and Zig DLL-backed `core.step` when Windows/WebView2 build dependencies are available |
| `native/linux/` | partial | C/GTK4/WebKitGTK scaffold with secure custom scheme loading that maps runtime resources to `runtime-web/` and generated app resources to `webapps/examples/`, reply-capable runtime bridge envelope handling, SQLite-backed storage through a `PlatformDatabase` opener that applies checked-in SQLite migrations and runs `PRAGMA integrity_check`, native open/save file dialogs, manifest-gated network requests, native permission checks, `core.step` through a dlopen-backed Zig C ABI loader when `libzig_core.so` is available, structured unsupported responses for unfinished platform services, and optional Meson/Xvfb smoke coverage for runtime load, bridge-backed storage persistence, and Zig shared-library `core.step` when Linux GTK/WebKitGTK dependencies are available |
| `codex-plugin/platform-control/` | partial | `plugin.json`, `.mcp.json`, skills present; local MCP path resolves to the repo server |
| `devtools/control-plane/` | partial | `openapi.json` + README |

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
| `tests/fixtures/bridge/` | partial (26 fixtures are exercised by fake-host bridge tests and Zig server bridge contract tests: the 16 docs/08 required fixtures plus network policy edge cases) |
| `tests/fixtures/capabilities/` | partial (schema-shaped runtime capability fixtures for fake-host, server, macOS, iOS simulator, Android, Windows, and Linux are exercised by runtime capability contract tests) |
| `tests/fixtures/db/` | partial |
| `tests/fixtures/snapshots/` | fixtures |
| `tests/golden/` | fixtures (5 golden flows) |
| `tests/micro/` | fixtures (5 micro-tests, one per bundled example app) |
| `tests/mutation/` | partial (19 mutations now exercised by fake-host validator/runtime tests) |
| `tests/accessibility/` | fixtures |
| `tests/performance/` | partial (fake-host latency harness reports p50/p95 for storage and core bridge round-trips) |
| `tests/security/malicious-packages/` | partial (12 fixtures exercised by fake-host security tests; static rejection plus runtime-denied storage-prefix and budget paths) |
| `tests/db/` | partial (8 checked-in dbtest fixtures are exercised by fake-host DB contract tests) |
| `tests/platform-smoke/` | fixtures (cross-platform suite now exercised by fake-host runner) |
| `tests/server/` | fixtures (mdok smoke verifies the Zig server starts and serves `/health` plus `/core/step`) |

## CI

Initial remote CI is wired in `.github/workflows/ci.yml` around `tools/check-repo.mjs`, Zig core/server tests, fake-host tests, fake-host performance smoke, Codex MCP contract tests, plus Linux GTK/WebKitGTK and Windows WebView2 native launch smoke jobs. `docs/12_RELEASE_AND_CI.md` describes the full matrix. First CI gates:

1. **JSON validate** — every JSON in the repo parses; every fixture validates against its schema.
2. **SQLite migrate** — `db/sqlite/*.sql` applies cleanly to an in-memory SQLite, required tables present.
3. **Postgres lint** — `db/postgres/*.sql` is checked for SQLite/Postgres table and logical-column parity, JSONB use, app-storage primary key shape, and optional live apply when `POSTGRES_TEST_URL` is set.
4. **Canonical example packages** — `webapps/examples/` is the only generated app package source.
5. **Spec lint** — section numbering contiguous; no `addJavascriptInterface` in native source; no `networkAllowlist` in manifests.
6. **Zig tests** — install Zig 0.15.2 and SQLite headers, then run `zig build test` in `zig-core/` and `server/`.
7. **Performance smoke** — run the fake-host latency harness with reduced CI samples to verify p50/p95 reporting and the storage/core control path.
8. **Linux native smoke** — install GTK4/WebKitGTK/Meson/Xvfb dependencies on `ubuntu-24.04`, build the Linux host, launch it under Xvfb/DBus, and verify runtime load, bridge-backed storage persistence, and `libzig_core.so` backed `core.step`.
9. **Windows native smoke** — download the pinned WebView2 SDK package on `windows-2022`, build the C++/WinRT host with CMake, launch it, and verify runtime load, bridge-backed storage persistence, and `zig_core.dll` backed `core.step`.

## How to update this file

1. Touch a file. 2. If the file's status changed (skeleton → partial, partial → complete, etc.), edit the row in the same commit. 3. If a new file/directory was added that has a status row's worth of meaning, add a row. 4. If status hasn't changed, leave the file alone — `git blame` is enough.
