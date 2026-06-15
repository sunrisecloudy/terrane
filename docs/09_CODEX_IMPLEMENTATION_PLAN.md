# Codex Implementation Plan

> **⚠️ SUPERSEDED (2026-06-12):** This was the v0.4 Zig/WebView implementation plan. New implementation work follows `prd-merged/` and the Forge task ledger.

## 1. Implementation principle

Build the platform in thin vertical slices. Do not generate all platforms in full at once. First prove the contract in the reference host (the reference implementation), then port the same contract to the Zig server, then to one desktop shell, then to the remaining shells.

This plan reorders earlier drafts to make the **reference host** the first end-to-end milestone after the Zig core. That way every native shell has a working byte-for-byte reference to diff against, and AI repair loops can run without any native toolchain.

## 2. Milestone 0 — repository skeleton

Deliverables:

- Monorepo directories per docs/02.
- Shared schemas.
- Runtime web placeholder directory.
- Example app packages committed.
- Zig core placeholder.
- Test fixture directories.
- `IMPLEMENTATION_STATUS.md` at the root.
- `LICENSE` at the root.

Acceptance:

- Repository opens cleanly.
- Examples validate against `schemas/manifest.schema.json`.
- Documentation files are present and consistent (no `networkAllowlist`, no `addJavascriptInterface`).
- `IMPLEMENTATION_STATUS.md` reflects current state.

## 3. Milestone 1 — Zig core

Deliverables:

- `zig-core` library.
- `core_create`, `core_destroy`, `core_step_json`, `core_free`.
- Demo event handling for the 5 example apps.
- Zig unit tests.
- FFI header (`include/zig_core.h`) checked in.

Acceptance:

- `zig build test` passes.
- Valid `core.step` fixture returns valid JSON.
- Replay determinism test passes.
- Pinned Zig toolchain version in `build.zig.zon`.

## 4. Milestone 2 — Reference host (contract) **[reordered]**

Deliverables (`tools/reference-host/`):

- Node process serving runtime + examples + bridge per docs/32.
- SQLite-backed `PlatformDatabase` (in-memory by default).
- Bridge dispatcher implementing every method in docs/03 §3.
- Dev control plane on `/control/*` with token auth (docs/14 §Authentication).
- Mock registries for `network.request`, `dialog.openFile`, `dialog.saveFile`, `notification.toast`.
- Zig core loaded via WebAssembly or Node addon.

Acceptance:

- `node tools/reference-host` starts and serves `/health`.
- All 5 examples install (reference-host may use dev signing; real native dev hosts use local Ed25519 signing), enable, and pass their bundled smoke tests.
- All bridge contract fixtures under `tests/fixtures/bridge/` pass.
- The control plane refuses requests without `X-Platform-Control-Token`.

This is the milestone after which every other host is a port. Don't move past it without all contract fixtures green.

## 5. Milestone 3 — Web runtime (browser-mock dev path)

Deliverables (`runtime-web/`):

- Runtime launcher (`index.html`).
- App registry reading bundled examples.
- Sandbox manager with per-mount `MessageChannel` and `mount_token` (docs/03 §2.1).
- Permission manager.
- Quota / budget meter.
- Debug console.
- Browser-only mock host (`window.__APP_RUNTIME_DEV_MOCK__ = true`).

Acceptance:

- Opening `runtime-web/index.html` locally displays the launcher.
- All 5 examples load in sandbox.
- Mock storage persists in memory for a session.
- Unknown method is denied.
- The same runtime, served by the reference host, runs the same way.

## 6. Milestone 4 — Package validator and signer

Deliverables (`tools/validate-webapp-package/`, signer integrated into the reference host):

- Manifest schema validation.
- Package structure validation.
- Static HTML/CSS/JS policy checks.
- Network policy + budget validation.
- Canonicalization + Ed25519 signing (docs/17).
- Install report generation.
- Smoke test runner against the reference host.

Acceptance:

- All bundled examples pass validation and produce a valid install report.
- Every mutation fixture under `tests/mutation/` fails validation with the expected error code.
- Tampered package fails mount.

## 7. Milestone 5 — Server shell

Deliverables (`server/`):

- Zig server executable.
- `/health`, `/bridge`, `/core/step`, `/webapps/examples`, `/webapps/validate`.
- SQLite for dev; Postgres adapter compiled but not required for v0.1.
- Contract tests.

Acceptance:

- `zig build run-server` starts.
- Contract fixtures pass byte-identically to the reference host.

## 8. Milestone 6 — macOS shell **[first native]**

Deliverables (`native/macos/`):

- Swift WKWebView app.
- Runtime bundle copy.
- Web bridge via `WKScriptMessageHandlerWithReply`.
- Zig core static library binding.
- `PlatformDatabase` (SQLite) implementation.
- Native open/save dialogs.
- Dev control plane on `127.0.0.1`.

Acceptance:

- macOS app launches and loads all examples.
- `core.step` and storage work.
- Contract fixtures match the reference host byte-identically.

## 9. Milestone 7 — iOS shell

Deliverables (`native/ios/`):

- Swift WKWebView app.
- iOS simulator target.
- Zig static library for simulator.
- Bridge/storage/core.step.
- App Store distribution build that ships only the 5 bundled apps (per docs/00 D1).
- TestFlight / Developer-ID build path that supports the full control plane.

Acceptance:

- iOS simulator runs examples.
- Storage persists across relaunch.
- App Store build refuses non-bundled installs.

## 10. Milestone 8 — Android shell

Deliverables (`native/android/`):

- Kotlin WebView app.
- Assets for runtime/examples.
- JNI bridge to Zig shared library.
- Bridge via `WebViewCompat.addWebMessageListener` with origin allowlist (docs/05 §4).
- `PlatformDatabase` (SQLite via AndroidX SQLite).
- Static check that fails the build if `addJavascriptInterface` is referenced on the runtime WebView.

Acceptance:

- Android emulator runs examples.
- JNI `core.step` works for arm64-v8a and x86_64 debug builds.
- Static check rejects `addJavascriptInterface`.

## 11. Milestone 9 — Windows shell

Deliverables (`native/windows/`):

- C++/WinRT WebView2 app.
- Zig DLL loading.
- Runtime/examples loaded via `SetVirtualHostNameToFolderMapping`.
- `PlatformDatabase` (vendored SQLite).

Acceptance:

- Windows app launches examples.
- `core.step` works.
- Contract fixtures match the reference host.

## 12. Milestone 10 — Linux shell

Deliverables (`native/linux/`):

- GTK4 + WebKitGTK app.
- Zig shared library loading.
- `PlatformDatabase` (system SQLite).

Acceptance:

- Linux app launches examples.
- `core.step` works.
- Contract fixtures match the reference host.

## 13. Milestone 11 — Cross-platform hardening

Deliverables:

- Full CI matrix (docs/12 §4).
- Contract tests across every bridge implementation.
- Security fixtures (malicious-packages).
- Performance smoke tests with p50/p95 reporting (docs/22 §7).
- Debug log export.

Acceptance:

- All platforms build or have documented local build commands.
- All examples smoke-test successfully.
- All performance p95s within target.

## 14. Codex rules

Codex must not:

- Introduce npm runtime dependencies for generated webapps.
- Replace generated app format with React/Vite/TypeScript.
- Expose raw native APIs to generated apps.
- Add platform-specific bridge methods without updating docs/03 and the schemas.
- Put business logic in native shells.
- Let native shells bypass permission checks.
- Use `addJavascriptInterface` on the runtime WebView (docs/05 §4).
- Use `manifest.networkAllowlist` (docs/00 D6; use `networkPolicy`).
- Read `appId` from request bodies (docs/03 §2.1).

Codex should:

- Keep generated webapps build-free.
- Keep bridge payloads JSON in v0.1–v0.4.
- Prefer small files with explicit responsibilities.
- Add tests with every module.
- Keep examples working after every milestone.
- Update `IMPLEMENTATION_STATUS.md` when crossing a status line.

## Codex control plugin milestones

### MCP-0: Add repository guidance

- Root `AGENTS.md` reflects current rules.
- Platform-specific guidance under `runtime-web/`, `zig-core/`, `native/`, and `tools/codex-platform-mcp/`.
- Codex must always run validators after editing generated app packages.

### MCP-1: Dev control protocol

- Implement `devtools/control-plane/` protocol types from `openapi.json`.
- Implement HTTP endpoints:
  - `GET /health`
  - `POST /sessions`
  - `DELETE /sessions/:id`
  - `GET /sessions/:id/snapshot`
  - `POST /sessions/:id/command`
  - `GET /sessions/:id/events`
- WebSocket event stream is optional; HTTP polling is acceptable for v0.2.
- Token authentication per docs/14 §Authentication.

### MCP-2: Runtime test hooks

- Dev-only runtime hooks for DOM snapshot, bridge log, console log, event log.
- Stable `data-testid` conventions on every example app.
- App reset support.

### MCP-3: MCP server

- Implement `tools/codex-platform-mcp`.
- Expose lifecycle, package, UI, runtime, bridge, core, storage, network-mock, timer, assertion, and replay tools (docs/14).
- Destructive tools require explicit `confirm: true`.

### MCP-4: Codex plugin packaging

- Plugin at `codex-plugin/platform-control`.
- Include `.codex-plugin/plugin.json`.
- Include `.mcp.json` pointing to the local MCP server.
- Include skills: `platform-micro-test`, `generated-webapp-repair`, `core-replay-debug`.

### MCP-5: Platform adapters

- Desktop: direct localhost connection.
- Android emulator: adapter that launches app and forwards control-plane port.
- iOS simulator: adapter that launches app with a dev control token and establishes a control session.
- Server: direct HTTP control API.

### MCP-6: Cross-platform suite

- `tests/micro/*.microtest.json` for every example.
- `tests/platform-smoke/*.json`.
- Run desktop smoke locally first, then mobile simulator/emulator when toolchains are available.

## v0.3 implementation milestones

### Milestone 12 — Trust / install pipeline

Deliverables:

- Package canonicalizer.
- Hash calculator.
- Ed25519 signature generator/verifier.
- Per-host platform keypair management (docs/17 §7).
- Install report generator.
- Immutable installed-version registry.

Acceptance:

- All example apps install through the signing path.
- Tampered packages fail mount.
- Install reports validate against `schemas/install-report.schema.json`.

### Milestone 13 — Versioning, rollback, migrations

Deliverables:

- App version records.
- Active-version pointer.
- Rollback command.
- Quarantine command.
- Migration runner with pre-migration snapshots and the operation grammar in docs/19 §5.

Acceptance:

- Failed app update rolls back automatically.
- Data version migration fixtures pass and fail correctly.

### Milestone 14 — Capabilities, budgets, network policy

Deliverables:

- Runtime capabilities API (docs/03 §8).
- Resource-budget enforcement.
- Host-mediated network policy implementation (docs/24).

Acceptance:

- All platform targets expose capabilities.
- Example apps adapt to missing optional capabilities.
- Network mutation tests fail safely.

### Milestone 15 — Snapshot/replay, Codex repair loop

Deliverables:

- Snapshot/restore tools.
- Replay tools.
- Accessibility report (docs/23).
- Codex repair-loop command sequence.

Acceptance:

- Reference host can reproduce a failing micro-test from snapshot.
- Codex can patch and retest a generated app through MCP tools.

## 15. v0.4 database implementation milestones

### DB-0: Schema files and fixtures

- Add SQLite migrations.
- Add Postgres-compatible migrations.
- Add DB JSON schemas and test fixtures.
- Add in-memory SQLite schema test.

### DB-1: Reference-host persistence

- Reference-host SQLite database service.
- App install transaction (one transaction across all install tables).
- `storage.get/set/list/remove` against `app_storage`.
- Persist bridge calls, core events/actions, runtime snapshots, test runs.

### DB-2: Package registry and rollback

- `apps`, `app_versions`, `app_files`, `app_permissions`, `app_installations` repositories.
- Activate, disable, rollback, list versions.
- Install report persistence.

### DB-3: Migrations and backup

- Declarative migration dry-run/apply per docs/19.
- Pre-migration snapshot.
- Backup export/import JSON.
- Export/import round-trip tests.

### DB-4: Codex DB tools

- MCP / control-plane tools: `db.snapshot`, `db.query_app_storage`, `db.query_app_versions`, `db.query_bridge_calls`, `db.query_core_events`, `db.query_test_runs`, `db.export_debug_bundle`.
- Confirm Codex cannot run arbitrary SQL by default.

### DB-5: Native and server adoption

- Port `PlatformDatabase` to every native host.
- Server uses SQLite in dev and Postgres-compatible logical schema in production.
- Platform smoke tests verify storage persistence after app restart.
