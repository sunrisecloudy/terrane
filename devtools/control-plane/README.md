# Dev Control Plane

The dev control plane is a local-only API exposed by native host dev builds and by the reference host.

It lets Codex, CI, and local test runners control the platform at micro-test granularity.

## Endpoint shape

```text
GET  /health
POST /sessions
DEL  /sessions/:id
GET  /sessions/:id/snapshot
GET  /sessions/:id/events
POST /command
```

`POST /command` is the main endpoint used by the MCP server.

Request:

```json
{
  "tool": "runtime.click",
  "args": {
    "sessionId": "session_123",
    "testId": "save-button"
  }
}
```

Response:

```json
{
  "ok": true,
  "result": {},
  "diagnostics": {
    "target": "macos",
    "sessionId": "session_123",
    "appId": "notes-lite",
    "timestamp": "2026-05-28T00:00:00Z"
  }
}
```

## Security

- Dev/test builds only.
- Localhost by default.
- Bearer token required.
- Destructive operations require `confirm: true`.
- Production builds must not bind this server.

## Reference host

Implement a reference host before platform-specific hosts. It should emulate package install, runtime bridge, storage, logs, and micro-tests so the MCP server can be tested in CI.

## v0.4 database inspection endpoints

The control plane must expose safe DB inspection endpoints or equivalent `POST /command` tools:

```text
POST /db/snapshot
POST /db/app-storage
POST /db/app-versions
POST /db/bridge-calls
POST /db/core-events
POST /db/test-runs
POST /db/export-backup
POST /db/import-backup
POST /db/export-debug-bundle
```

These are dev/test-only and must require the control token. They must not expose arbitrary SQL by default.
