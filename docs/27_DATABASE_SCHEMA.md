# Database Schema

## 1. Goal

The platform must include a formal persistence layer that is boring, portable, inspectable, and compatible across native hosts, the fake host, server development, and production server deployments.

The database stores:

- generated app registry records;
- immutable generated app versions;
- generated app package files;
- version-scoped permissions;
- active installations;
- app storage key/value data;
- install reports;
- migrations and migration runs;
- runtime/debug logs;
- bridge/core event logs;
- runtime snapshots and replay data;
- Codex micro-tests, test runs, control sessions, mocks, and commands;
- backup/export/import records.

## 2. Storage architecture

```text
Native hosts: SQLite
Fake host: SQLite
Local development: SQLite
Server development: SQLite
Server production: Postgres-compatible logical schema
```

SQLite is the default for iOS, Android, macOS, Windows, Linux, local dev, CI fake host, and smoke tests. The server must support the same logical tables with Postgres-compatible DDL for production.

## 3. Ownership model

There are two layers:

```text
Platform DB
  owned by native host/platform runtime/server

Generated app storage
  exposed only through AppRuntime.call("storage.*")
```

Generated apps do not create SQL tables, execute SQL, or access database handles. They only call:

```js
AppRuntime.call("storage.get", ...)
AppRuntime.call("storage.set", ...)
AppRuntime.call("storage.list", ...)
AppRuntime.call("storage.remove", ...)
```

Internally, these calls map to:

```text
app_storage(app_id, key, value_json)
```

## 4. Required table groups

### App registry

| Table | Purpose |
|---|---|
| `apps` | One logical generated app, active install pointer, lifecycle status |
| `app_versions` | Immutable installed package versions and content hashes |
| `app_files` | Versioned package file contents or asset refs |
| `app_permissions` | Version-scoped permissions and approval status |
| `app_installations` | Install/activate/rollback/disable history |

### Generated app storage

| Table | Purpose |
|---|---|
| `app_storage` | Namespaced JSON key/value data for generated apps |

`app_storage` is the most important table for generated app persistence:

```sql
CREATE TABLE app_storage (
  app_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value_json TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (app_id, key)
);
```

Every storage key must be prefixed with the app storage prefix declared in the manifest, such as `notes-lite:`.

### Runtime/debug logs

| Table | Purpose |
|---|---|
| `runtime_sessions` | A host/runtime launch or test session |
| `bridge_calls` | Every bridge call made by generated apps in debug/test mode |
| `core_events` | Events submitted to Zig core |
| `core_actions` | Actions returned by Zig core |
| `runtime_snapshots` | Serializable snapshot/replay records |

### Testing/control

| Table | Purpose |
|---|---|
| `micro_tests` | Stored micro-test specs |
| `test_runs` | Test execution results and diagnostics |
| `control_sessions` | Codex/dev-control sessions |
| `control_commands` | Control-plane commands issued during tests |
| `network_mocks` | Per-session network mock definitions |
| `dialog_mocks` | Per-session file dialog mock definitions |

### Versioning/migration

| Table | Purpose |
|---|---|
| `app_migrations` | Declarative app data migrations |
| `migration_runs` | Dry-run/apply execution records |
| `app_install_reports` | Validation, security, compatibility, and smoke-test install reports |
| `backup_exports` | Export/import/debug bundle metadata |

## 5. Table contract

The canonical SQL files live in:

```text
db/sqlite/001_initial.sql
db/sqlite/002_runtime_debug.sql
db/sqlite/003_codex_control.sql
db/sqlite/004_migrations_and_snapshots.sql

db/postgres/001_initial.sql
db/postgres/002_runtime_debug.sql
db/postgres/003_codex_control.sql
db/postgres/004_migrations_and_snapshots.sql
```

Implementation rule: when adding a platform feature that persists state, update both SQLite and Postgres schema files, then update the relevant `schemas/db-*.schema.json` document.

## 6. Transaction rules

App install must be one transaction:

```text
BEGIN
  validate package
  insert/update apps
  insert app_versions
  insert app_files
  insert app_permissions
  insert app_install_reports
  insert app_installations
  activate version if approved
COMMIT
```

Rollback must be one transaction:

```text
BEGIN
  verify target version exists and is not quarantined
  update apps.active_install_id
  insert app_installations action=rollback
  optionally create runtime snapshot
COMMIT
```

Storage writes must be atomic per key. Multi-key migrations must run inside a transaction and create a `migration_runs` record.

## 7. Compatibility notes

SQLite stores JSON as `TEXT`; Postgres stores equivalent values as `JSONB`. The logical shape is the same. Implementations must not rely on platform-specific JSON query features in core business logic unless an equivalent path exists for both databases.

## 8. Indexing requirements

Minimum indexes:

```text
app_versions(app_id, version)
app_files(install_id, path)
app_permissions(install_id, permission)
app_storage(app_id, updated_at)
bridge_calls(session_id, created_at)
core_events(session_id, created_at)
core_actions(event_id, created_at)
runtime_snapshots(session_id, created_at)
test_runs(session_id, started_at)
control_commands(control_session_id, created_at)
```

## 9. Generated app storage invariants

- `app_storage.app_id` must equal the owning generated app id.
- `app_storage.key` must begin with the app `storagePrefix`.
- Values are JSON text in v1.
- No generated app can list or read another app's keys.
- App data survives runtime restarts, host app restarts, and app version upgrades unless a migration explicitly changes it.
