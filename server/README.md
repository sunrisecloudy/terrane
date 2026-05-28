# Zig Server Target

Minimal Zig server that uses the same core FFI as native hosts.

Minimum endpoints:

```text
GET  /health
POST /core/step
GET  /webapps/examples
POST /webapps/validate
```

MVP acceptance:

- `zig build run-server` starts the server.
- `/health` returns OK.
- `/core/step` accepts the same JSON event shape as native bridge calls.
- Contract fixtures pass.

Current local run command:

```sh
zig build run-server -- --port 8088
```

Implemented endpoints:

- `GET /health`
- `POST /core/step`
- `POST /bridge` for `core.step`, `runtime.capabilities`, `storage.*`, mock-backed dialogs, `notification.toast`, mock-backed `network.request`, `app.log`, and structured bridge errors
- `POST /webapps/validate` for server-side package shape and static policy validation
- `POST /webapps/install` for token-gated inline package validation and transactional SQLite install
- `POST /packages/validate`, `/packages/sign`, and `/packages/policy-audit` for token-gated package control helpers
- `POST /apps/{appId}/rollback` for token-gated package rollback to a previous installed version
- `GET /webapps/examples`
- `POST /control/command` for token-gated package validation/signing/install/rollback, snapshot create/restore, migration dry-run/apply, network mock setup/reset, dialog mock setup, app registry queries, `platform.health`, `runtime.capabilities`, and safe `db.*` inspection commands
- `POST /db/snapshot`, `/db/app-storage`, `/db/app-versions`, `/db/bridge-calls`, `/db/core-events`, `/db/test-runs`, and `/db/export-debug-bundle` for token-gated safe DB inspection

## v0.4 persistence requirement

Server dev persistence initializes the v0.4 logical SQLite schema: app registry/install tables, `app_storage`, runtime/debug tables, Codex control tables, test/mocking tables, migrations, install reports, and backup export metadata. Package signing validates the inline source package, calculates package hashes, and emits a schema-shaped Ed25519 signature plus `contentHashesDocument`; set `NATIVE_AI_SERVER_SIGNING_SEED` to make the local signing key host-specific. Package installs use that signing path, enforce the manifest `runtimeVersion` compatibility rule from docs/04 §8, statically evaluate bundled `smoke-tests.json`, write a `test_runs` record, quarantine incompatible or failed-smoke installs, and commit `apps`, `app_versions`, `app_files`, `app_permissions`, `app_install_reports`, and `app_installations` in one SQLite transaction. The `/bridge` path derives app identity from headers, rejects incompatible active `runtimeVersion`, rejects unapproved bridge permissions against the active install, enforces the active manifest `resourceBudget` for bridge-call, network-request, log-line, and storage-byte limits, supports `notification.toast` as an audited server no-op, serves `network.request` only from registered `network_mocks` after enforcing the active manifest `networkPolicy`, and serves `dialog.openFile` / `dialog.saveFile` from registered `dialog_mocks` with `dialog.saveFile` defaulting to `{ "ok": true }` when no mock is registered. Activated updates with higher `dataVersion` must include contiguous `migrations/<from>_to_<to>.json` files; the install transaction applies them, records `app_migrations`/`migration_runs`, and rejects incomplete chains. Package rollback is transactional: it verifies a non-quarantined target, refuses incompatible `dataVersion` rollbacks unless `snapshotId` is supplied for data restore, flips `apps.active_install_id`, marks the former version `rolled-back`, enables the target, and writes `app_installations.action='rollback'`. Runtime snapshots capture active app identity, storage, bridge/core logs, capabilities, and resource counters with a `sha256:` content hash; restore replaces the app storage namespace and active install pointer from the saved snapshot. Migration dry-run/apply creates a pre-migration snapshot, persists `app_migrations` and `migration_runs`, and applies deterministic key-level storage changes before bumping `apps.data_version`. Successful `runtime.capabilities`, `core.step`, `storage.*`, mock-backed dialogs, `notification.toast`, and mock-backed `network.request` bridge calls write `bridge_calls`; `core.step` also persists submitted events and returned actions to `core_events`/`core_actions`; `app.log` validates `level`/`message`, writes a redacted `bridge_calls` row, and mirrors the message to stderr. By default the server writes `server-platform.sqlite` in the current working directory; set `NATIVE_AI_SERVER_DB=/path/to/platform.sqlite` to choose another file.

Safe DB inspection endpoints and `/control/command` require `X-Platform-Control-Token` to match the per-launch control token. In dev mode the server writes a 32-byte URL-safe token to `control.token`; override the path with `--token-file`, `NATIVE_AI_SERVER_CONTROL_TOKEN_FILE`, or `PLATFORM_CONTROL_TOKEN_FILE`, and override the token itself only for tests with `NATIVE_AI_SERVER_CONTROL_TOKEN` or `PLATFORM_CONTROL_TOKEN`. Three failed attempts from the same client address trigger a 60-second `control_connection_banned` response that is also audited. They expose fixed read-only queries only; arbitrary SQL is not available. Accepted and rejected control requests are audited into `control_commands` under a host-owned `server-control-audit` session. Debug bundle exports include a SHA-256 `contentHash` and are recorded in `backup_exports`. The server creates host-owned app rows for bundled-example storage/log writes so `app_storage` can keep its relational `apps(id)` boundary without generated apps choosing SQL state.

Set `NATIVE_AI_SERVER_ENV=production` to disable dev/control endpoints such as `/control/command`, `/db/*`, `/packages/*`, `/webapps/install`, and app rollback routes. In production mode the server also rejects dev-only startup flags `--control-plane-port`, `--allow-runtime-mismatch`, `--allow-unsigned-dev`, and `--token-file`.

Remaining persistence work: browser-backed smoke-test execution and the Postgres production adapter.
