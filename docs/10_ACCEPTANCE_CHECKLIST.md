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
- [x] Runtime exposes `window.__APP_RUNTIME_DEVTOOLS__` only in dev/test mode.

## Zig core

- [x] Zig core builds.
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
- [x] Each app has smoke tests using `data-testid` selectors.
- [x] Each app uses only `AppRuntime.call`.

## Platform shells

### iOS

- [x] iOS simulator launches.
- [x] Runtime loads from bundle.
- [x] Bridge works.
- [x] Storage persists.
- [x] Zig core step works.
- [x] Bundled app index exposes content ratings and enforces the iOS age gate.
- [x] iOS bridge rejects app ids outside the bundled app index.

### macOS

- [x] macOS app launches.
- [x] Runtime loads from bundle.
- [x] Bridge works.
- [x] Storage persists.
- [x] Storage bridge failures return structured `storage_error` responses.
- [x] WebView content-process termination records a failed runtime session and offers a reload action.
- [x] Dialogs work or return structured unsupported errors.
- [x] Zig core step works.
- [x] `core.step` times out with structured `timeout` errors without blocking the WebView reply path.
- [x] Production guard rejects and audits dev-only startup flags outside DEBUG builds.
- [x] Debug dev control plane writes a 0600 token file, authenticates health/session/snapshot/events/command routes, and audits accepted/rejected requests.
- [x] Debug dev control plane exposes `runtime.capabilities` through session and command routes.
- [x] Debug dev control plane exposes DB-backed `runtime.resource_usage` through session and command routes.
- [x] Debug dev control plane exposes static accessibility audit/snapshot/assertion tools.
- [x] Debug dev control plane supports static `runtime.query`, `runtime.assert_visible`, and `runtime.assert_text` over generated app HTML.
- [x] Debug dev control plane supports static `runtime.screenshot`, `runtime.click`, `runtime.type`, `runtime.set_value`, `runtime.press_key`, and `runtime.drag` controls.
- [x] Debug dev control plane supports static `runtime.wait_for` and no-op test-mode `runtime.timer_advance`.
- [x] Debug dev control plane exposes safe `db.snapshot` and fixed `db.query_*` inspection without arbitrary SQL.
- [x] Debug dev control plane exports `db.export_debug_bundle` artifacts and records them in `backup_exports`.
- [x] Debug dev control plane exports/imports portable `db.export_backup` / `db.import_backup` documents and records export/import rows in `backup_exports`.
- [x] Debug dev control plane persists, reads, and restores runtime snapshots for app storage.
- [x] Debug dev control plane compares runtime snapshots with normalized stable hashes through `runtime.compare_snapshot`.
- [x] Debug dev control plane lists installed webapps/versions and rolls back app registry versions.
- [x] Debug dev control plane quarantines and uninstalls webapps with confirmation-gated destructive uninstall and a pre-uninstall snapshot.
- [x] Debug dev control plane rejects `platform.reset_webapp` and `runtime.storage_reset` without `confirm: true`.
- [x] Debug dev control plane approval-gates permission/policy/capability-changing app updates and activates them only through `platform.approve_webapp_update`.
- [x] Debug dev control plane approval-gates `dataVersion`-changing app updates and applies packaged migrations before activation.
- [x] Debug dev control plane supports runtime storage get/set plus confirmation-gated reset, bridge-call inspection/assertion, and log clearing.
- [x] Debug dev control plane supports lifecycle target list/launch/reload and opening installed webapps into runtime sessions.
- [x] Debug dev control plane supports `runtime.call_bridge` and `runtime.core_step` through permission-checked bridge dispatch.
- [x] Debug dev control plane quarantines an active install after three resource-budget bridge violations in 60 seconds and restores the previous active install.
- [x] Debug dev control plane supports `runtime.replay_events` with a fresh deterministic Zig core replay.
- [x] Debug dev control plane supports `runtime.assert_storage`, `runtime.core_snapshot`, and DB-backed `runtime.assert_core_action` for storage/core-log assertions.
- [x] Debug dev control plane captures `notification.toast` calls through `runtime.notification_capture`.
- [x] Debug dev control plane supports DB-backed `runtime.network_mock_set` / `runtime.network_mock_reset` and `runtime.dialog_mock_set` for mock-backed bridge calls.
- [x] Debug dev control plane supports DB-backed one-shot `runtime.fault_inject` for bridge calls.
- [x] Debug dev control plane runs bundled static smoke tests through `runtime.run_smoke_tests` and persists `test_runs`.
- [x] Debug dev control plane signs webapp packages with Ed25519, not `none-dev`.
- [x] Debug dev control plane persists its Ed25519 signing key in Keychain and exposes public-key metadata.
- [x] Debug dev control plane verifies active installed package signatures/content before opening webapps.
- [x] Debug dev control plane validates, signs, installs, and runs checked-in static micro-tests through `runtime.run_microtest`.
- [x] Debug dev control plane runs the checked-in cross-platform smoke suite through `platform.run_platform_smoke`.
- [x] Debug dev control plane runs declarative migration dry-run/apply with pre-migration snapshots and persisted `migration_runs`.

### Android

- [x] Android emulator launches.
- [x] Runtime loads from assets.
- [x] Bridge works.
- [x] JNI Zig core step works.
- [x] Storage persists.

### Windows

- [ ] Windows app launches.
- [ ] WebView2 loads runtime.
- [ ] Bridge works.
- [ ] Zig DLL loads.
- [ ] Storage persists.

### Linux

- [x] GTK/WebKitGTK app launches.
- [x] Runtime loads resources.
- [x] Bridge works.
- [x] `notification.toast` validates message/level params against the bridge contract.
- [x] Zig shared library loads.
- [x] Storage persists.
- [x] Production guard rejects and audits dev-only startup flags in release builds.

### Server

- [x] Server starts.
- [x] `/health` works.
- [x] `/core/step` works.
- [x] Contract tests pass.

## Security

- [x] Unknown bridge method denied.
- [x] Missing permission denied.
- [x] Cross-app storage key denied.
- [x] Malicious package fixtures rejected.
- [x] Direct network use, resource hints, external HTML resources, and non-package CSS URLs rejected by validator.
- [x] Remote scripts rejected.
- [x] Quota exceeded path tested.
- [x] Native bridges reject `appId` in bridge params and use the channel-derived app id.

## Tests

- [x] Zig tests pass.
- [x] Runtime unit tests pass.
- [x] Package validator tests pass.
- [x] Bridge contract tests pass.
- [x] Example smoke tests pass.
- [x] Bundled smoke tests use `data-testid` selectors and install-time-safe step/assertion vocabulary.
- [x] Platform smoke tests pass or are documented for manual execution.


## Codex control acceptance

- [x] A local Codex plugin skeleton exists at `codex-plugin/platform-control`.
- [x] The plugin contains `.codex-plugin/plugin.json`, `.mcp.json`, and at least three skills.
- [x] The MCP server exposes tools for launch, install, open, inspect, interact, assert, mock, replay, and reset.
- [x] The MCP server exposes per-tool input schemas and rejects invalid or unconfirmed destructive calls before forwarding.
- [x] A dev host can be launched with a control token.
- [x] Codex can install and open all five example webapps through the control plane.
- [x] Codex can click/type/assert inside generated apps using `data-testid` selectors.
- [x] Codex can inspect `app.log`-backed console logs, bridge calls, runtime events, storage, and core action logs.
- [x] Codex can mock at least one network response and one file dialog result.
- [x] Codex can capture `notification.toast` calls through persisted bridge logs.
- [x] Codex can run every example app smoke test from `tests/micro`.
- [x] Production builds do not expose the control plane.

## v0.3 acceptance checklist additions

- [x] All example manifests include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy`.
- [x] All example packages validate against updated schemas.
- [x] Package install creates a signature and install report.
- [x] Fake host persists a configured Ed25519 platform key file and exposes public key metadata.
- [x] Runtime refuses tampered installed packages.
- [x] App registry stores immutable versions and active-version pointer.
- [x] Rollback works on fake-host and at least one desktop target.
- [x] Runtime capabilities API works on every target.
- [x] Snapshot/replay works on fake-host.
- [x] Resource-budget violations are detected.
- [x] Network policy blocks disallowed requests.
- [x] Accessibility audit runs in fake-host.
- [x] Codex repair loop can validate, install, test, patch, and retest an example app.

## v0.4 database persistence acceptance

- [x] SQLite schema exists under `db/sqlite`.
- [x] Postgres-compatible schema exists under `db/postgres`.
- [x] SQLite migrations apply cleanly to an in-memory database.
- [x] Native hosts use SQLite by default.
- [x] Server can use SQLite/Postgres logical schema.
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
