# Database Schema

## 1. Goal

The platform must include a formal persistence layer that is boring, portable, inspectable, and compatible across native hosts, the reference host, server development, and production server deployments.

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
- platform-owned notebook CRDT documents, updates, actors, permissions, proposals, and sync cursors;
- Codex micro-tests, test runs, control sessions, mocks, and commands;
- backup/export/import records.

## 2. Storage architecture

```text
Native hosts: SQLite
Reference host: SQLite
Local development: SQLite
Server development: SQLite
Server production: Postgres-compatible logical schema
```

SQLite is the default for iOS, Android, macOS, Windows, Linux, local dev, CI reference host, and smoke tests. The server must support the same logical tables with Postgres-compatible DDL for production.

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

### Notebook CRDT **[CRDT]**

Notebook CRDT persistence is platform-owned. Generated apps never access these tables directly; they use `AppRuntime.call("notebook.*", ...)` and the host derives `app_id`, actor identity, and notebook access from sandbox/session context.

| Table | Purpose |
|---|---|
| `crdt_notebooks` | One logical notebook per app with lifecycle status and creator metadata |
| `crdt_documents` | Verified materialized notebook snapshots by version/content hash |
| `crdt_updates` | Append-only accepted/rejected operation audit with actor, sequence, status, error code, and hash |
| `crdt_heads` | Current frontier/version/content hash for a notebook |
| `crdt_actors` | Human, AI, and system actors known to an app |
| `crdt_permissions` | Notebook ACL rows by actor and notebook permission |
| `crdt_proposals` | AI proposal records and review status |
| `crdt_sync_cursors` | Per-actor sync cursor/frontier metadata |

SQLite stores notebook JSON fields as `TEXT`; Postgres stores equivalent logical fields as `JSONB`. Required notebook JSON fields are `metadata`, `cells`, `comments`, `aiRuns`, `proposals`, and `approvals`.

## 5. Table contract

The canonical SQL files live in:

```text
db/sqlite/001_initial.sql
db/sqlite/002_runtime_debug.sql
db/sqlite/003_codex_control.sql
db/sqlite/004_migrations_and_snapshots.sql
db/sqlite/005_crdt_notebooks.sql

db/postgres/001_initial.sql
db/postgres/002_runtime_debug.sql
db/postgres/003_codex_control.sql
db/postgres/004_migrations_and_snapshots.sql
db/postgres/005_crdt_notebooks.sql
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

Notebook update apply must be transactional:

```text
BEGIN
  validate installed app permission
  validate notebook ACL and actor policy
  validate operation schema
  replay accepted updates plus candidate operation
  validate materialized notebook schema
  insert crdt_updates row with status=accepted or status=rejected
  on acceptance, insert crdt_documents snapshot and upsert crdt_heads
  on proposal create/decision, upsert crdt_proposals
COMMIT
```

Imported sync updates must be idempotent by `opId`: duplicate accepted operations return duplicate status without changing materialized state. Destructive compaction, migration, rollback, or import must create a snapshot first and preserve replay/convergence guarantees.

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
crdt_updates(app_id, notebook_id, seq)
crdt_updates(app_id, notebook_id, json-extracted opId)
crdt_updates(status, created_at)
crdt_documents(app_id, notebook_id, version)
crdt_permissions(app_id, actor_id, permission)
crdt_sync_cursors(app_id, notebook_id, actor_id)
```

## 9. Generated app storage invariants

- `app_storage.app_id` must equal the owning generated app id.
- `app_storage.key` must begin with the app `storagePrefix`.
- Values are JSON text in v1.
- No generated app can list or read another app's keys.
- App data survives runtime restarts, host app restarts, and app version upgrades unless a migration explicitly changes it.

## 10. Notebook CRDT invariants **[CRDT]**

- `crdt_notebooks.app_id` is derived from sandbox/control context; generated apps cannot choose another app id.
- Every `crdt_updates` row belongs to exactly one `(app_id, notebook_id)` pair and records the derived actor id/kind.
- Accepted updates are replayable in `(seq, update_id)` order; rejected updates remain audit records and do not affect materialized state.
- `crdt_documents` snapshots are derived from accepted updates and include a content hash for stable comparison.
- `crdt_heads.frontier_json` is the current supported frontier representation for `notebook.snapshot`, `notebook.checkout`, and sync cursors.
- `crdt_permissions` is checked in addition to installed manifest permissions; runtime preflight alone is not authoritative.
- AI actors default to `notebook.read`, `notebook.propose`, and `notebook.sync`; canonical writes require explicit trusted host policy and audit.
