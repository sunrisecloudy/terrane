# App Data Migrations

## 1. Purpose

Generated apps will evolve and their storage format will change. Data migrations make updates predictable and reversible enough for AI-assisted repair.

## 2. Manifest field

Every manifest must include:

```json
{ "dataVersion": 1 }
```

`dataVersion` is a positive integer (no semver). It increases by exactly 1 per shape change. The storage namespace remains `storagePrefix`.

## 3. Migration files

Optional migration files live under the package:

```text
migrations/
  1_to_2.json
  2_to_3.json
  ...
```

Each migration validates against `schemas/app-migration.schema.json`. Filenames must follow `<from>_to_<to>.json` where `<to> == <from> + 1`. The installer composes consecutive migrations to reach the target `dataVersion`.

## 4. Migration file shape

```json
{
  "fromDataVersion": 1,
  "toDataVersion": 2,
  "description": "Add createdAt to notes",
  "operations": [
    { "op": "setDefault", "keyPattern": "notes-lite:notes", "jsonPath": "$[*].createdAt", "value": null },
    { "op": "renameKey",  "from": "notes-lite:tags",        "to": "notes-lite:labels" }
  ],
  "preflight": {
    "requireKeys": ["notes-lite:notes"],
    "maxKeysAffected": 10000
  }
}
```

## 5. Declarative operation grammar

Allowed operations:

| Operation | Required fields | Effect |
|---|---|---|
| `setDefault` | `keyPattern`, `jsonPath`, `value` | For every storage key matching `keyPattern`, walk `jsonPath` and set `value` if the path is undefined. |
| `deleteJsonPath` | `keyPattern`, `jsonPath` | Remove the value at `jsonPath` inside the JSON value of every matching key. |
| `renameJsonPath` | `keyPattern`, `from`, `to` | Move a JSON value from path `from` to path `to` inside every matching key. |
| `transformEnum` | `keyPattern`, `jsonPath`, `mapping` | Replace enum values according to the `mapping` object. Unmapped values fail the migration unless `defaultMapping` is set. |
| `copyJsonPath` | `keyPattern`, `from`, `to` | Copy a JSON value from `from` to `to` without removing the source. |
| `renameKey` | `from`, `to` | Rename a single storage key. `to` must start with the app `storagePrefix`. |
| `deleteKey` | `key` | Remove a single storage key. |
| `moveStorageKey` | `from`, `to`, optional `mergeStrategy` | Move a storage key. If `to` exists, apply `mergeStrategy` (`overwrite`, `keep`, `mergeShallow`, `mergeDeep`). |
| `mapEachInArray` | `keyPattern`, `jsonPath`, `template` | Replace every element under `jsonPath` (which must point to an array) by interpolating `template` (the original value is bound as `$item`). |

`keyPattern` supports glob-style `*` and `?` and is anchored by app `storagePrefix`. JSON paths use the standard `$.foo[0].bar` syntax. Arbitrary JS migrations are not allowed in v0.3 or v0.4.

### 5.1 Operation validation

Validators must check:

- Every `keyPattern` starts with `<storagePrefix>*`.
- Every `from`/`to`/`key` starts with `<storagePrefix>`.
- `setDefault.value` is JSON-serializable.
- `transformEnum.mapping` keys and values are scalars.
- `maxKeysAffected` is ≥ the count of keys matched at preflight; otherwise the migration is rejected before any write.

### 5.2 Preflight

Every migration includes optional `preflight`:

| Field | Effect |
|---|---|
| `requireKeys` | Array of storage keys that must exist; missing keys fail preflight. |
| `forbidKeys` | Array of storage keys that must not exist; presence fails preflight. |
| `maxKeysAffected` | Soft cap on number of keys this migration may touch; refusal if exceeded. |
| `expectedDataVersion` | Sanity check on the current `dataVersion`. |

Preflight runs inside a read-only transaction. If preflight fails the migration is not applied and the previous version remains active.

## 6. Migration pipeline

```text
install new version
  -> verify package signature
  -> if manifest.dataVersion > stored dataVersion:
       for v in stored+1 .. new:
         load migrations/<v-1>_to_<v>.json
         validate operations against schema
         run preflight (read-only)
  -> create pre-migration snapshot (docs/21)
  -> BEGIN transaction
       for each migration v-1 -> v:
         insert app_migrations row
         apply operations to app_storage
         insert migration_runs row (status = running)
       update stored dataVersion
       activate new version
     COMMIT
  -> run smoke tests
  -> if smoke tests pass: enable
     else: restore snapshot, quarantine new version, keep old active
```

On any failure inside the transaction:

```text
ROLLBACK
restore pre-migration snapshot
quarantine new version
keep old version active
insert migration_runs row (status = failed, error_code, error_message)
```

## 7. Migration report

Every migration run writes (and persists to `migration_runs`):

```json
{
  "runId": "mrun_...",
  "appId": "notes-lite",
  "fromDataVersion": 1,
  "toDataVersion": 2,
  "status": "success",
  "startedAt": "2026-05-28T10:00:00Z",
  "completedAt": "2026-05-28T10:00:00.412Z",
  "changedKeys": ["notes-lite:notes"],
  "operationCounts": { "setDefault": 12, "renameKey": 1 },
  "snapshotId": "snap_...",
  "error": null
}
```

## 8. Rollback rules

- **Down-migration without script**: not supported. Rolling back to a lower `dataVersion` requires restoring a snapshot taken before the up-migration. The installer creates this snapshot automatically.
- **Down-migration with script**: an optional `migrations/down/<to>_to_<from>.json` file may exist. If present, the host may apply it during a manual rollback. It uses the same grammar as up-migrations.
- **Rollback target has higher `dataVersion` than current**: refuse with `rollback_data_version_incompatible`.

## 9. Codex rules

Codex must not change storage shape in generated app code without also updating:

- `manifest.dataVersion`;
- migration files when existing user data is affected;
- smoke tests or micro-tests that cover old and new data.

If Codex changes shape without writing a migration, the package validator must fail the install with `data_version_unchanged` or `migration_missing`.

## 10. Database-backed migration records **[v0.4]**

Migrations are stored in `app_migrations`; executions are stored in `migration_runs`. Every apply has a pre-migration snapshot where possible. Failed migrations write a failed `migration_runs` row and keep the previous active version.

Schema cross-reference: see `schemas/app-migration.schema.json` and the `app_migrations` / `migration_runs` definitions in `db/sqlite/004_migrations_and_snapshots.sql` and `db/postgres/004_migrations_and_snapshots.sql`.
