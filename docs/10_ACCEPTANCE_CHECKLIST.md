# Acceptance Checklist v0.1

## Repository

- [x] Monorepo structure exists.
- [x] Documentation exists under `docs/`.
- [x] Schemas exist under `schemas/`.
- [x] Example apps exist under `webapps/examples/`.
- [x] Runtime web project exists.
- [x] Zig core project exists.
- [x] Native platform directories exist.
- [x] Server project exists.

## Runtime

- [x] Runtime launcher displays installed/bundled apps.
- [x] Runtime can load each app in sandbox.
- [x] Runtime exposes `AppRuntime.call`.
- [x] Runtime rejects unknown methods.
- [x] Runtime enforces manifest permissions.
- [x] Runtime enforces storage prefixes.
- [x] Runtime logs bridge calls in debug console.
- [x] Runtime shows structured errors.

## Zig core

- [ ] Zig core builds.
- [x] Zig core tests pass.
- [x] FFI API works.
- [x] `core.step` accepts valid JSON event.
- [x] `core.step` returns valid JSON actions.
- [x] Invalid input does not crash.
- [x] Replay is deterministic.

## Example apps

- [x] Notes Lite loads.
- [x] Task Workbench loads.
- [x] File Transformer loads.
- [x] API Dashboard loads.
- [x] Core Replay Lab loads.
- [x] Each app declares permissions.
- [x] Each app has smoke tests.
- [x] Each app uses only `AppRuntime.call`.

## Platform shells

### iOS

- [ ] iOS simulator launches.
- [ ] Runtime loads from bundle.
- [ ] Bridge works.
- [ ] Storage persists.
- [ ] Zig core step works.

### macOS

- [ ] macOS app launches.
- [ ] Runtime loads from bundle.
- [ ] Bridge works.
- [ ] Dialogs work or return structured unsupported errors.
- [ ] Zig core step works.

### Android

- [ ] Android emulator launches.
- [ ] Runtime loads from assets.
- [ ] Bridge works.
- [ ] JNI Zig core step works.
- [ ] Storage persists.

### Windows

- [ ] Windows app launches.
- [ ] WebView2 loads runtime.
- [ ] Bridge works.
- [ ] Zig DLL loads.
- [ ] Storage persists.

### Linux

- [ ] GTK/WebKitGTK app launches.
- [ ] Runtime loads resources.
- [ ] Bridge works.
- [ ] Zig shared library loads.
- [ ] Storage persists.

### Server

- [ ] Server starts.
- [ ] `/health` works.
- [ ] `/core/step` works.
- [ ] Contract tests pass.

## Security

- [x] Unknown bridge method denied.
- [x] Missing permission denied.
- [x] Cross-app storage key denied.
- [x] Malicious package fixtures rejected.
- [x] Direct network use rejected by validator.
- [x] Remote scripts rejected.
- [x] Quota exceeded path tested.

## Tests

- [ ] Zig tests pass.
- [x] Runtime unit tests pass.
- [x] Package validator tests pass.
- [x] Bridge contract tests pass.
- [x] Example smoke tests pass.
- [ ] Platform smoke tests pass or are documented for manual execution.


## Codex control acceptance

- [x] A local Codex plugin skeleton exists at `codex-plugin/platform-control`.
- [x] The plugin contains `.codex-plugin/plugin.json`, `.mcp.json`, and at least three skills.
- [x] The MCP server exposes tools for launch, install, open, inspect, interact, assert, mock, replay, and reset.
- [x] A dev host can be launched with a control token.
- [x] Codex can install and open all five example webapps through the control plane.
- [x] Codex can click/type/assert inside generated apps using `data-testid` selectors.
- [x] Codex can inspect console logs, bridge calls, runtime events, storage, and core action logs.
- [x] Codex can mock at least one network response and one file dialog result.
- [x] Codex can run every example app smoke test from `tests/micro`.
- [x] Production builds do not expose the control plane.

## v0.3 acceptance checklist additions

- [x] All example manifests include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy`.
- [x] All example packages validate against updated schemas.
- [x] Package install creates a signature and install report.
- [x] Runtime refuses tampered installed packages.
- [x] App registry stores immutable versions and active-version pointer.
- [ ] Rollback works on fake-host and at least one desktop target.
- [ ] Runtime capabilities API works on every target.
- [x] Snapshot/replay works on fake-host.
- [x] Resource-budget violations are detected.
- [x] Network policy blocks disallowed requests.
- [x] Accessibility audit runs in fake-host.
- [ ] Codex repair loop can validate, install, test, patch, and retest an example app.

## v0.4 database persistence acceptance

- [x] SQLite schema exists under `db/sqlite`.
- [x] Postgres-compatible schema exists under `db/postgres`.
- [x] SQLite migrations apply cleanly to an in-memory database.
- [ ] Native hosts use SQLite by default.
- [ ] Server can use SQLite/Postgres logical schema.
- [x] Generated apps use storage bridge only; no SQL APIs are exposed.
- [x] App install transaction writes `apps`, `app_versions`, `app_files`, `app_permissions`, `app_install_reports`, and `app_installations`.
- [x] App data persists in `app_storage`.
- [x] Storage keys are scoped by `app_id` and `storagePrefix`.
- [x] Permission versioning is persisted per install id.
- [x] Rollback can restore previous app version.
- [x] Bridge/core logs are persisted.
- [x] Runtime snapshots are persisted.
- [x] Micro-test runs are persisted.
- [x] Declarative migration dry-run/apply works.
- [x] Backup export/import works for one generated app.
- [x] Codex can inspect DB state through safe control-plane tools.
- [x] Codex cannot run arbitrary SQL unless unsafe dev mode is explicitly enabled.
- [x] Database tests under `tests/db` pass.
