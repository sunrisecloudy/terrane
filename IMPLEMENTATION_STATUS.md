# Implementation Status

This is the single source of truth for what is built vs planned. Update this file in the same commit as any change that crosses a status line.

Legend:

- **spec-only** — doc exists; no code/skeleton.
- **schema** — JSON schema present; no implementation.
- **skeleton** — README or placeholder directory only.
- **fixtures** — sample data / fixtures present.
- **partial** — implementation under way but not feature-complete.
- **complete** — implementation passes the contract tests for its surface.

Status snapshot: **2026-05-28**.

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
| `docs/13_EXAMPLE_APP_COVERAGE.md` | spec-only (thin — needs expansion) | qa |
| `docs/14_CODEX_CONTROL_PLUGIN.md` | spec-only (v0.4) | codex |
| `docs/15_MICRO_TESTING_PROTOCOL.md` | spec-only (v0.4) | qa |
| `docs/16_CODEX_PLUGIN_IMPLEMENTATION_PLAN.md` | spec-only | codex |
| `docs/17_APP_SIGNING_AND_TRUST.md` | spec-only (v0.4) | platform |
| `docs/18_APP_VERSIONING_AND_ROLLBACK.md` | spec-only | platform |
| `docs/19_DATA_MIGRATIONS.md` | spec-only (v0.4) | platform |
| `docs/20_RUNTIME_CAPABILITIES.md` | spec-only | runtime |
| `docs/21_SNAPSHOT_AND_REPLAY_FORMAT.md` | spec-only | runtime |
| `docs/22_RESOURCE_BUDGETS.md` | spec-only (v0.4) | runtime |
| `docs/23_ACCESSIBILITY_CONTRACT.md` | spec-only (thin — needs tooling decision) | runtime |
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
| `runtime-web/` | partial | Launcher HTML/CSS/JS exists; fake host serves it and proxies sandbox `AppRuntime.call` messages to `/bridge` or WebKit native bridge with `AppRuntime.on` events, per-mount nonce/port binding, permission, storage-prefix, network-policy, and rate-budget preflight |
| `zig-core/` | partial | Zig 0.15.2 static library package with C FFI header, deterministic JSON core.step behavior, and unit tests |
| `server/` | partial | Minimal Zig HTTP server with `/health`, `/core/step`, `/bridge` for core/capabilities/storage/app.log, `/webapps/validate`, `/webapps/examples`, and token-gated safe `/db/*` inspection endpoints; uses Zig core FFI and SQLite-backed dev `app_storage` plus redacted `app.log` bridge records |
| `tools/fake-platform-host/` | partial | Node fake host with SQLite migrations, session/command/package/app/db control HTTP surface, package validation/install with smoke-test, runtime-compatibility, and update-approval gating, Ed25519 signing, verified mount gate, version rollback/quarantine, snapshots, migrations, backup export/import, bridge dispatch with resource-budget checks, static runtime controls, static smoke/micro-test runner, and focused tests |
| `tools/codex-platform-mcp/` | partial | Dependency-free MCP stdio JSON-RPC server forwards declared tools to fake-host/dev control plane; node:test coverage present |
| `native/ios/` | partial | SwiftPM/UIKit/WKWebView scaffold with WKScriptMessageHandlerWithReply, runtime bridge envelope handling, SQLite-backed storage, manifest-gated network requests, native permission checks, and structured unsupported responses for unfinished platform services |
| `native/macos/` | partial | SwiftPM AppKit/WKWebView host scaffold with runtime bridge envelope handling, SQLite storage, dialogs, manifest-gated network requests, toast logging, and structured bridge responses |
| `native/android/` | partial | Kotlin Android scaffold with generated shared runtime/example assets, WebViewAssetLoader, WebViewCompat message bridge with origin allowlist, SQLite-backed storage, manifest-derived native context, manifest-gated network requests, native permission checks, and structured unsupported responses for unfinished platform services |
| `native/windows/` | partial | C++/WinRT/WebView2 scaffold with virtual-host loading, WebMessageReceived origin checks, SQLite-backed storage, manifest-gated network requests, native permission checks, and structured unsupported responses for unfinished platform services |
| `native/linux/` | partial | C/GTK4/WebKitGTK scaffold with secure custom scheme loading, script-message bridge, SQLite-backed storage, manifest-gated network requests, native permission checks, and structured unsupported responses for unfinished platform services |
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
| `examples/` (root) | **deprecated duplicate** — canonical path is `webapps/examples/`. Plan to consolidate. |

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
| `tests/fixtures/bridge/` | partial (fixtures are now exercised by fake-host bridge tests) |
| `tests/fixtures/db/` | partial |
| `tests/fixtures/snapshots/` | fixtures |
| `tests/golden/` | fixtures (5 golden flows) |
| `tests/micro/` | fixtures (5 micro-tests, one per bundled example app) |
| `tests/mutation/` | partial (10 mutations now exercised by fake-host validator/runtime tests) |
| `tests/accessibility/` | fixtures |
| `tests/security/malicious-packages/` | partial (static validator fixtures exercised by fake-host tests) |
| `tests/db/` | fixtures (DB test fixtures) |
| `tests/platform-smoke/` | fixtures (cross-platform suite now exercised by fake-host runner) |

## CI

Initial remote CI is wired in `.github/workflows/ci.yml` around `tools/check-repo.mjs`, fake-host tests, and Codex MCP contract tests. `docs/12_RELEASE_AND_CI.md` describes the full matrix. First CI gates:

1. **JSON validate** — every JSON in the repo parses; every fixture validates against its schema.
2. **SQLite migrate** — `db/sqlite/*.sql` applies cleanly to an in-memory SQLite, required tables present.
3. **Postgres lint** — `db/postgres/*.sql` is static-checked; full apply runs when a Postgres service is available.
4. **Example duplicate sync** — `examples/` and `webapps/examples/` package files must match (until consolidation).
5. **Spec lint** — section numbering contiguous; no `addJavascriptInterface` in native source; no `networkAllowlist` in manifests.

## How to update this file

1. Touch a file. 2. If the file's status changed (skeleton → partial, partial → complete, etc.), edit the row in the same commit. 3. If a new file/directory was added that has a status row's worth of meaning, add a row. 4. If status hasn't changed, leave the file alone — `git blame` is enough.
