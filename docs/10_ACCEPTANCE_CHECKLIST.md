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
- [x] Runtime sets generated app iframe CSP and no feature-policy delegations.
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
- [x] `notification.toast` validates message/level params against the bridge contract.
- [x] iOS simulator app bundle build verifies all five bundled example apps include manifest, HTML, CSS, and JS resources; debug simulator smoke includes an all-example bridge capability probe when launch smoke is enabled.
- [x] iOS debug simulator smoke verifies native storage reset creates a manual pre-reset `runtime_snapshots` row and clears storage through the real bridge.
- [x] Source/static checks verify iOS DEBUG simulator dev control first slice starts only in dev mode, binds a loopback token-gated `GET /health` endpoint, writes a 0600 per-launch control token file, and persists accepted/rejected SQLite audit rows.
- [x] Source/static checks verify iOS DEBUG simulator dev control exposes lightweight session/control routes for `platform.list_targets`, `platform.list_webapps`, bridge-routed `runtime.capabilities`, `runtime.call_bridge`, and `runtime.core_step` with runtime session persistence.
- [x] Source/static checks verify iOS DEBUG simulator dev control exposes safe `db.snapshot`, fixed `db.query_*`, and `db.export_debug_bundle` controls through command and `/db/*` routes without arbitrary SQL.
- [x] Source/static and simulator build checks verify iOS DEBUG simulator dev control routes `runtime.storage_get`, `runtime.storage_set`, and `runtime.assert_storage` through the native bridge with storage-prefix enforcement and bridge-call logging, exposes confirmation-gated `runtime.storage_reset` / `platform.reset_webapp` with pre-reset `runtime_snapshots`, and clears runtime logs for `platform.reset_webapp`.
- [x] Source/static and simulator build checks verify iOS DEBUG simulator dev control supports DB-backed `runtime.resource_usage`, `runtime.event_log`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors`.
- [x] Source/static and simulator build checks verify iOS DEBUG simulator dev control supports fresh-core `runtime.replay_events` plus DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`.
- [x] Source/static and simulator build checks verify iOS DEBUG simulator dev control supports explicit `platform.create_snapshot`, confirmation-gated `platform.restore_snapshot`, and normalized `runtime.compare_snapshot` over app-storage runtime snapshots.
- [x] Source/static and simulator build checks verify iOS DEBUG simulator dev control supports static `runtime.accessibility_snapshot`, `runtime.run_accessibility_audit`, and `runtime.assert_accessibility` over bundled generated app HTML.

### macOS

- [x] macOS app launches.
- [x] Runtime loads from bundle.
- [x] Bridge works.
- [x] Storage persists.
- [x] Storage bridge failures return structured `storage_error` responses.
- [x] WebView content-process termination records a failed runtime session and offers a reload action.
- [x] Dialogs work or return structured unsupported errors.
- [x] Zig core step works.
- [x] `notification.toast` validates message/level params against the bridge contract.
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
- [x] `notification.toast` validates message/level params against the bridge contract.
- [x] Source/static checks verify Android WebView bridge hardening: `WEB_MESSAGE_LISTENER` feature gate, single internal origin allowlist, no `addJavascriptInterface`, file-URL access disabled, release debugging disabled, and Safe Browsing enabled.
- [x] Source/static and debug APK build checks verify Android `network.request` uses OkHttp with manifest policy enforcement and timeout clamping.
- [x] Source/static and debug APK build checks verify Android debug dev control first slice is debug-only, loopback-bound, private-token-file gated, SQLite-audited, exposes health/session/control routes, routes `runtime.capabilities` / `runtime.call_bridge` / `runtime.core_step` through the native bridge, and restricts DB inspection to allowlisted queries.
- [x] Source/static and debug APK build checks verify Android debug dev control supports `platform.list_targets` and `platform.list_webapps` with bundled app metadata through the token-gated command route.
- [x] Source/static and debug APK build checks verify Android debug dev control routes `runtime.storage_get`, `runtime.storage_set`, `runtime.assert_storage`, `runtime.resource_usage`, `runtime.event_log`, and `runtime.console_logs` through native bridge and fixed SQLite readers.
- [x] Source/static and debug APK build checks verify Android debug dev control exposes static `runtime.accessibility_snapshot`, `runtime.run_accessibility_audit`, and `runtime.assert_accessibility` over installed or bundled generated app HTML.
- [x] Source/static and debug APK build checks verify Android debug dev control supports DB-backed `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors`.
- [x] Source/static and debug APK build checks verify Android debug dev control supports confirmation-gated `runtime.storage_reset` / `platform.reset_webapp` with pre-reset `runtime_snapshots`.
- [x] Source/static and debug APK build checks verify Android debug dev control supports DB-backed one-shot `runtime.fault_inject` consumed before permission/budget bridge dispatch.
- [x] Source/static and debug APK build checks verify Android debug dev control supports DB-backed `runtime.network_mock_set` / `runtime.network_mock_reset` and `runtime.dialog_mock_set` consumed by Android network/dialog bridge calls.
- [x] Source/static and debug APK build checks verify Android debug dev control includes `runtime_snapshots` / `backup_exports` in safe `db.snapshot` output and supports `db.export_debug_bundle` with a persisted `backup_exports` row.
- [x] Source/static and debug APK build checks verify Android debug dev control exports/imports portable `db.export_backup` / `db.import_backup` documents over fixed app/package/storage tables and records `backup_exports` rows.
- [x] Source/static and debug APK build checks verify Android debug dev control supports `platform.create_snapshot`, confirmation-gated `platform.restore_snapshot`, and normalized `runtime.compare_snapshot` over app-storage runtime snapshots.
- [x] Source/static and debug APK build checks verify Android debug dev control supports fresh-core `runtime.replay_events` plus DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`.

### Windows

- [ ] Windows app launches.
- [ ] WebView2 loads runtime.
- [ ] Bridge works.
- [ ] Zig DLL loads.
- [ ] Storage persists.
- [x] Source/static checks verify Windows native `dialog.openFile` supports `multiple: true` through WebView2-hosted Win32 multi-select file dialogs.
- [ ] Debug dev control plane runtime-smoke verifies per-launch token file, loopback bind, token-gated `GET /health` plus session create/snapshot/events/capabilities/command/end routes, and accepted/rejected audit rows.
- [x] Source/static checks verify Windows debug dev control supports `platform.list_targets` and `platform.list_webapps` with bundled app metadata through the token-gated command route.
- [x] Source/static checks verify Windows debug dev control supports static HTML `runtime.screenshot`, `runtime.query`, target interaction, wait/timer, and visible/text assertion commands.
- [x] Source/static checks verify Windows debug dev control supports fresh-core `runtime.replay_events` plus DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`.
- [x] Source/static checks verify Windows debug dev control supports explicit `platform.create_snapshot`, confirmation-gated `platform.restore_snapshot`, and normalized `runtime.compare_snapshot` over app-storage runtime snapshots.
- [ ] Debug dev control plane runtime-smoke verifies `runtime.capabilities`, `runtime.call_bridge`, and `runtime.core_step` through permission-checked bridge dispatch with bridge/core DB logging.
- [ ] Debug dev control plane runtime-smoke verifies safe `db.snapshot` and fixed `db.query_*` inspection without arbitrary SQL.

### Linux

- [x] GTK/WebKitGTK app launches.
- [x] Runtime loads resources.
- [x] Bridge works.
- [x] `notification.toast` validates message/level params against the bridge contract.
- [x] Zig shared library loads.
- [x] Storage persists.
- [x] Production guard rejects and audits dev-only startup flags in release builds.
- [x] Debug dev control plane writes a 0600 per-launch token file, binds to loopback, token-gates `GET /health` plus session create/snapshot/events/capabilities/command/end routes, and audits accepted/rejected requests.
- [x] Debug dev control plane runtime-smoke verifies `platform.list_targets` and `platform.list_webapps` with bundled app metadata through the token-gated command route.
- [x] Debug dev control plane Docker-smoke verifies static HTML `runtime.screenshot`, `runtime.query`, target interaction, `runtime.wait_for`, `runtime.timer_advance`, and visible/text assertion commands over bundled app packages.
- [x] Debug dev control plane Docker-smoke verifies static HTML `runtime.accessibility_snapshot`, `runtime.run_accessibility_audit`, and `runtime.assert_accessibility` controls over bundled app packages.
- [x] Debug dev control plane supports `runtime.call_bridge` and `runtime.core_step` through permission-checked bridge dispatch with bridge/core DB logging.
- [x] Debug dev control plane Docker-smoke verifies fresh-core `runtime.replay_events` plus DB-backed `runtime.core_snapshot` and `runtime.assert_core_action`.
- [x] Debug dev control plane runtime-smoke verifies direct `runtime.storage_get`, `runtime.storage_set`, confirmation-gated `runtime.storage_reset` / `platform.reset_webapp`, pre-reset snapshots, and `runtime.assert_storage` with storage-prefix enforcement through the native bridge.
- [x] Debug dev control plane runtime-smoke verifies DB-backed `runtime.resource_usage`, `runtime.event_log`, and `runtime.console_logs`.
- [x] Debug dev control plane Docker-smoke verifies DB-backed `runtime.bridge_calls`, `runtime.clear_logs`, `runtime.notification_capture`, `runtime.assert_bridge_call`, and `runtime.assert_no_console_errors`.
- [x] Debug dev control plane runtime-smoke verifies DB-backed `runtime.network_mock_set` / `runtime.network_mock_reset` and `runtime.dialog_mock_set` for mock-backed bridge calls.
- [x] Debug dev control plane Docker-smoke verifies DB-backed one-shot `runtime.fault_inject` for bridge calls, including disabled-row evidence and bridge error audit.
- [x] Debug dev control plane runtime-smoke verifies safe `db.snapshot` and fixed `db.query_*` inspection without arbitrary SQL.
- [x] Debug dev control plane Docker-smoke verifies `db.export_debug_bundle` returns a hashed Linux debug bundle and persists a `backup_exports` row.
- [x] Debug dev control plane Docker-smoke verifies portable `db.export_backup` / `db.import_backup` over fixed app/package/storage tables and records export/import rows in `backup_exports`.
- [x] Debug dev control plane Docker-smoke verifies explicit `platform.create_snapshot`, confirmation-gated `platform.restore_snapshot`, and normalized `runtime.compare_snapshot` over app-storage runtime snapshots with persisted `runtime_snapshots` rows and audit evidence.
- [x] Linux native release package includes runtime, example app, SQLite migration, and Zig core resources.
- [x] Linux native release artifact launches from its packaged directory without repo-root resource assumptions.
- [x] Linux packaged host resolves runtime resources, app resources, migrations, and `libzig_core.so` relative to the executable.

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
- [x] Reference host persists a configured Ed25519 platform key file and exposes public key metadata.
- [x] Runtime refuses tampered installed packages.
- [x] App registry stores immutable versions and active-version pointer.
- [x] Rollback works on reference-host and at least one desktop target.
- [x] Runtime capabilities API works on every target.
- [x] Snapshot/replay works on reference-host.
- [x] Resource-budget violations are detected.
- [x] Network policy blocks disallowed requests.
- [x] Accessibility audit runs in reference-host.
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
