# Storage and Migrations

## 1. Generated app storage bridge

Generated apps persist data through the runtime bridge only:

```js
await AppRuntime.call("storage.get", { key, defaultValue })
await AppRuntime.call("storage.set", { key, value })
await AppRuntime.call("storage.list", { prefix })
await AppRuntime.call("storage.remove", { key })
```

The runtime enforces:

- declared `storage.read` / `storage.write` permissions;
- `storagePrefix` ownership;
- resource budget storage limits;
- structured logging to `bridge_calls`;
- persistence to `app_storage`.

Generated apps never create tables. They also never receive a database connection, SQL API, or raw filesystem path for app data.

## 2. Storage key model

Every key must start with the app prefix:

```text
notes-lite:notes
notes-lite:settings
notes-lite:documents/doc_123
```

Storage implementation:

```text
storage.set({ key, value })
  -> extract app_id from active app context
  -> require key starts with manifest.storagePrefix
  -> serialize value to JSON
  -> UPSERT app_storage(app_id, key, value_json, updated_at)
```

`storage.list` must scope by app id and prefix:

```sql
SELECT key
FROM app_storage
WHERE app_id = ? AND key LIKE ? || '%'
ORDER BY key ASC;
```

## 3. App versioning and storage

Installed generated app versions are immutable. Storage belongs to the logical app id, not to one install id, so app data can survive upgrades and rollbacks.

```text
apps.id = notes-lite
apps.active_install_id = install_notes_lite_2
app_storage.app_id = notes-lite
```

Version-specific package files and permissions live in `app_versions`, `app_files`, and `app_permissions`.

## 4. Migration model

Generated app data can change shape. The manifest declares `dataVersion` and optional migrations are bundled under `migrations/`.

v1 supports declarative JSON migrations only.

Example:

```json
{
  "id": "notes-lite-1-to-2",
  "appId": "notes-lite",
  "fromDataVersion": 1,
  "toDataVersion": 2,
  "steps": [
    {
      "op": "renameField",
      "collection": "notes",
      "from": "name",
      "to": "title"
    }
  ]
}
```

## 5. Supported v1 migration operations

| Operation | Description |
|---|---|
| `renameField` | Rename a field in each object in a collection key |
| `setDefault` | Add default value when field is missing |
| `deleteField` | Remove a field from objects |
| `renameKey` | Rename one storage key |
| `copyKey` | Copy one storage key |
| `deleteKey` | Delete one storage key |
| `transformEnum` | Map known enum values to new values |

All operations must be deterministic and serializable in JSON.

## 6. Migration execution flow

```text
install package with higher dataVersion
  -> validate migrations are present and contiguous
  -> create pre-migration runtime snapshot
  -> dry run migration in transaction/savepoint
  -> write migration_runs dry-run result
  -> if approved: apply migration in transaction
  -> update apps.data_version
  -> write migration_runs apply result
  -> activate new app version
```

Dry run and apply must return structured reports.

## 7. Rollback interaction

Rollback has two levels:

1. **App package rollback**: switches `apps.active_install_id` to an older version.
2. **Data rollback**: restores a pre-migration snapshot when possible.

For v1, every migration must create a pre-migration snapshot. Rollback of migrated data is allowed only if the snapshot is available and the user/dev control plane explicitly requests it.

## 8. Migration failure handling

On migration failure:

- do not activate the new app version;
- keep the previous active version;
- write an `app_install_reports` failure;
- write a `migration_runs` failure;
- preserve the pre-migration snapshot;
- surface diagnostics to Codex repair loop.

## 9. Codex repair integration

When migration validation fails, Codex receives:

```text
manifest dataVersion
current stored dataVersion
migration id
failed operation
sample offending key/value
install report id
snapshot id
```

Codex must patch migration JSON or app package files, then rerun validation and migration dry-run before install.
