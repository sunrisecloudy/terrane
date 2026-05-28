# Backup, Export, and Import

## 1. Goal

The platform must export a portable snapshot that can move between devices, reproduce bugs, feed Codex replay, and support future migration paths.

v1 export format is a single JSON file. Later versions may use a ZIP package for assets and large logs.

## 2. Export contents

Minimum export:

```text
runtime metadata
platform capabilities
installed apps
active app versions
app package manifests
app package files or file refs
app permissions
app storage
app migrations
install reports
```

Optional debug export additions:

```text
runtime sessions
bridge calls
core events
core actions
runtime snapshots
test runs
control commands
network/dialog mocks
```

## 3. Export schema

The canonical schema is `schemas/backup-export.schema.json`.

Top-level shape:

```json
{
  "exportId": "export_001",
  "createdAt": "2026-05-28T00:00:00Z",
  "runtimeVersion": "0.4.0",
  "source": { "platform": "fake-host" },
  "apps": [],
  "appVersions": [],
  "appFiles": [],
  "appPermissions": [],
  "appStorage": [],
  "runtimeCapabilities": {},
  "debug": {}
}
```

## 4. Export types

| Type | Includes logs/tests? | Use case |
|---|---:|---|
| `backup` | No | Device transfer and restore |
| `debug-bundle` | Yes | Bug reports, Codex repair, replay |
| `test-fixture` | Selected | CI fixtures and deterministic tests |

## 5. Import flow

```text
receive export JSON
  -> validate backup-export schema
  -> validate content hashes/signatures when present
  -> validate runtime compatibility
  -> create import transaction
  -> insert apps/app_versions/app_files/app_permissions
  -> insert app_storage
  -> optionally insert logs/snapshots/test history
  -> activate app versions according to export policy
  -> write import install report
```

## 6. Conflict policy

When importing an app that already exists:

| Condition | Default behavior |
|---|---|
| Same app id + same content hash | Skip duplicate |
| Same app id + newer version | Install inactive; ask before activate |
| Same app id + older version | Install inactive as rollback candidate |
| Same key in app_storage | Keep local unless import policy says overwrite |
| Permission increase | Requires approval |

## 7. Debug bundles for Codex

`db.export_debug_bundle` must create a compact bundle containing:

- active app version;
- app manifest and package files;
- relevant `app_storage` rows;
- relevant bridge/core logs;
- runtime snapshot;
- runtime capability info;
- test run diagnostics.

This bundle is the preferred input for Codex bug reproduction.

## 8. Security rules

- Import must never bypass package validation.
- Import must never activate an app that requests new permissions without approval.
- Import must not restore bridge logs into production telemetry unless explicitly requested.
- Import must reject malformed JSON, invalid signatures, and oversized payloads.
