# Database Test Plan

## 1. Scope

Database tests cover SQLite schema creation, Postgres logical schema parity, generated app storage, app install transactions, rollback, migration, runtime/debug logging, Codex control persistence, and backup/export/import.

## 2. Test levels

```text
Schema tests
  -> apply SQL migrations
  -> verify expected tables/indexes

Storage tests
  -> storage.get/set/list/remove bridge behavior
  -> app prefix isolation

Transaction tests
  -> app install
  -> app activation
  -> rollback
  -> migration dry-run/apply/failure

Logging tests
  -> bridge call insert
  -> core event/action insert
  -> runtime snapshot insert
  -> test run insert

Export/import tests
  -> backup round trip
  -> debug bundle round trip
  -> conflict handling

Corruption tests
  -> malformed JSON
  -> missing app version
  -> invalid active_install_id
  -> interrupted migration
```

## 3. Required DB test fixtures

```text
tests/db/sqlite-schema.dbtest.json
tests/db/postgres-schema.dbtest.json
tests/db/app-install-transaction.dbtest.json
tests/db/storage-crud.dbtest.json
tests/db/rollback.dbtest.json
tests/db/migration-dry-run-apply.dbtest.json
tests/db/backup-export-import.dbtest.json
tests/db/corruption-handling.dbtest.json
```

## 4. SQLite schema tests

Use an in-memory SQLite database in CI:

```text
sqlite3 :memory:
  read db/sqlite/001_initial.sql
  read db/sqlite/002_runtime_debug.sql
  read db/sqlite/003_codex_control.sql
  read db/sqlite/004_migrations_and_snapshots.sql
  assert required tables exist
  assert required indexes exist
```

## 5. Postgres schema tests

For v1, Postgres tests may be static in CI if no Postgres container is configured. Required static checks:

- all required tables exist in SQL text;
- JSON columns use `JSONB`;
- logical column names match SQLite where possible;
- app storage primary key is `(app_id, key)`;
- no platform-only table is missing from Postgres.

When a Postgres test container is available, migrations must apply cleanly.

## 6. App install transaction test

Given a valid package:

1. start transaction;
2. insert `apps` row if missing;
3. insert `app_versions` row;
4. insert `app_files` rows for manifest/index/styles/app/smoke-tests;
5. insert `app_permissions` rows;
6. insert `app_install_reports` row;
7. insert `app_installations` row;
8. update active install id;
9. commit.

Assertions:

- all rows exist;
- active install id points to the new version;
- files can be reconstructed;
- permissions are scoped to install id.

## 7. Storage CRUD test

Assertions:

- `storage.set` upserts `app_storage`;
- `storage.get` returns parsed JSON;
- `storage.list` returns only keys for the same app id and prefix;
- `storage.remove` deletes one key;
- another app cannot read the key.

## 8. Rollback test

Install v1 and v2 of an app. Activate v2. Roll back to v1.

Assertions:

- `apps.active_install_id` points to v1;
- both version records remain immutable;
- rollback installation event exists;
- permissions reflect the active install;
- app storage remains intact unless data rollback is explicitly requested.

## 9. Migration test

Install v1 with data version 1, write storage, install v2 with migration to data version 2.

Assertions:

- dry run creates no final data change;
- apply updates app storage;
- migration run row records status;
- failed migration does not activate new app version;
- pre-migration snapshot exists.

## 10. Backup/export/import test

Create one app with data, permissions, bridge logs, core logs, and a snapshot. Export backup JSON. Import into a clean SQLite database.

Assertions:

- schema validation passes;
- active app version is restored;
- app storage is restored;
- optional debug logs restore only when requested;
- duplicate import is idempotent.

## 11. Corruption handling tests

- Invalid JSON in `app_storage.value_json` returns structured error and does not crash.
- Missing active app version prevents app mount.
- Interrupted migration can be detected from `migration_runs` and snapshot state.
- DB open failure shows host-level error, not generated app error.
