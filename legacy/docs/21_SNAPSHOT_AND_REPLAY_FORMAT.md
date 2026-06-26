# Snapshot and Replay Format

## 1. Purpose

Snapshots make bugs reproducible across platforms and across Codex sessions. A snapshot captures the app package, runtime state, bridge/core logs, storage namespace, and platform capabilities.

Use `schemas/runtime-snapshot.schema.json`.

## 2. Snapshot contents

A snapshot should include:

- snapshot id and timestamp;
- platform/target/runtime version;
- active app id and installed version id;
- manifest and package hashes;
- runtime capabilities;
- current route/screen;
- DOM summary or accessibility tree;
- namespaced storage data;
- bridge call log;
- core event/action log;
- console errors/logs;
- network/dialog/timer mocks;
- resource usage;
- screenshot reference when available.

## 3. Replay model

```text
snapshot
  -> restore app version
  -> restore storage
  -> restore mocks
  -> replay event log
  -> compare bridge/core/actions/visible UI
```

A replay result must report:

- same outputs;
- divergent outputs;
- missing capabilities;
- nondeterministic behavior;
- runtime errors.

## 4. Snapshot types

| Type | Purpose |
|---|---|
| `bug-report` | User or Codex bug reproduction |
| `pre-install` | Before enabling a new app version |
| `pre-migration` | Before data migration |
| `post-test` | After smoke/micro test run |
| `golden` | Regression baseline |

## 5. Privacy

Snapshots may contain user data. The platform must support redaction:

```json
{
  "redactStorageValues": true,
  "redactNetworkBodies": true,
  "redactTextContent": false
}
```

Codex should request non-redacted snapshots only in trusted local development.

## 6. Control-plane tools

Required tools:

- `platform.create_snapshot`;
- `platform.restore_snapshot`;
- `runtime.replay_events`;
- `runtime.compare_snapshot`.

## Database snapshot persistence

Runtime snapshots are persisted in `runtime_snapshots` with `snapshot_json` and `content_hash`. Backup/debug exports can include these snapshots for Codex replay. Pre-migration snapshots should be linked from `migration_runs.pre_snapshot_id`.
