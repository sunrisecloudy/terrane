# Acceptance Checklist v0.1

## Repository

- [ ] Monorepo structure exists.
- [ ] Documentation exists under `docs/`.
- [ ] Schemas exist under `schemas/`.
- [ ] Example apps exist under `webapps/examples/`.
- [ ] Runtime web project exists.
- [ ] Zig core project exists.
- [ ] Native platform directories exist.
- [ ] Server project exists.

## Runtime

- [ ] Runtime launcher displays installed/bundled apps.
- [ ] Runtime can load each app in sandbox.
- [ ] Runtime exposes `AppRuntime.call`.
- [ ] Runtime rejects unknown methods.
- [ ] Runtime enforces manifest permissions.
- [ ] Runtime enforces storage prefixes.
- [ ] Runtime logs bridge calls in debug console.
- [ ] Runtime shows structured errors.

## Zig core

- [ ] Zig core builds.
- [ ] Zig core tests pass.
- [ ] FFI API works.
- [ ] `core.step` accepts valid JSON event.
- [ ] `core.step` returns valid JSON actions.
- [ ] Invalid input does not crash.
- [ ] Replay is deterministic.

## Example apps

- [ ] Notes Lite loads.
- [ ] Task Workbench loads.
- [ ] File Transformer loads.
- [ ] API Dashboard loads.
- [ ] Core Replay Lab loads.
- [ ] Each app declares permissions.
- [ ] Each app has smoke tests.
- [ ] Each app uses only `AppRuntime.call`.

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

- [ ] Unknown bridge method denied.
- [ ] Missing permission denied.
- [ ] Cross-app storage key denied.
- [ ] Malicious package fixtures rejected.
- [ ] Direct network use rejected by validator.
- [ ] Remote scripts rejected.
- [ ] Quota exceeded path tested.

## Tests

- [ ] Zig tests pass.
- [ ] Runtime unit tests pass.
- [ ] Package validator tests pass.
- [ ] Bridge contract tests pass.
- [ ] Example smoke tests pass.
- [ ] Platform smoke tests pass or are documented for manual execution.


## Codex control acceptance

- [ ] A local Codex plugin skeleton exists at `codex-plugin/platform-control`.
- [ ] The plugin contains `.codex-plugin/plugin.json`, `.mcp.json`, and at least three skills.
- [ ] The MCP server exposes tools for launch, install, open, inspect, interact, assert, mock, replay, and reset.
- [ ] A dev host can be launched with a control token.
- [ ] Codex can install and open all five example webapps through the control plane.
- [ ] Codex can click/type/assert inside generated apps using `data-testid` selectors.
- [ ] Codex can inspect console logs, bridge calls, runtime events, storage, and core action logs.
- [ ] Codex can mock at least one network response and one file dialog result.
- [ ] Codex can run every example app smoke test from `tests/micro`.
- [ ] Production builds do not expose the control plane.

## v0.3 acceptance checklist additions

- [ ] All example manifests include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy`.
- [ ] All example packages validate against updated schemas.
- [ ] Package install creates a signature and install report.
- [ ] Runtime refuses tampered installed packages.
- [ ] App registry stores immutable versions and active-version pointer.
- [ ] Rollback works on fake-host and at least one desktop target.
- [ ] Runtime capabilities API works on every target.
- [ ] Snapshot/replay works on fake-host.
- [ ] Resource-budget violations are detected.
- [ ] Network policy blocks disallowed requests.
- [ ] Accessibility audit runs in fake-host.
- [ ] Codex repair loop can validate, install, test, patch, and retest an example app.

## v0.4 database persistence acceptance

- [ ] SQLite schema exists under `db/sqlite`.
- [ ] Postgres-compatible schema exists under `db/postgres`.
- [ ] SQLite migrations apply cleanly to an in-memory database.
- [ ] Native hosts use SQLite by default.
- [ ] Server can use SQLite/Postgres logical schema.
- [ ] Generated apps use storage bridge only; no SQL APIs are exposed.
- [ ] App install transaction writes `apps`, `app_versions`, `app_files`, `app_permissions`, `app_install_reports`, and `app_installations`.
- [ ] App data persists in `app_storage`.
- [ ] Storage keys are scoped by `app_id` and `storagePrefix`.
- [ ] Permission versioning is persisted per install id.
- [ ] Rollback can restore previous app version.
- [ ] Bridge/core logs are persisted.
- [ ] Runtime snapshots are persisted.
- [ ] Micro-test runs are persisted.
- [ ] Declarative migration dry-run/apply works.
- [ ] Backup export/import works for one generated app.
- [ ] Codex can inspect DB state through safe control-plane tools.
- [ ] Codex cannot run arbitrary SQL unless unsafe dev mode is explicitly enabled.
- [ ] Database tests under `tests/db` pass.
