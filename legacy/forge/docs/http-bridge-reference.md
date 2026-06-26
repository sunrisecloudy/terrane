# Forge HTTP Bridge Reference

`forge-server` exposes a minimal JSON HTTP surface over one in-process `WorkspaceCore`. It is the reference transport for web consoles, embedded tools, and integration tests.

## Routes

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/health` | Liveness JSON (`service`, `console` flag). |
| `GET` | `/docs` | Generated public API reference (`forge/docs/public-api/index.html`). |
| `GET` | `/docs/*` | Static assets for the docs page (`styles.css`, `app.js`). |
| `GET` | `/console` | Catalog-driven operator console UI. |
| `GET` | `/schemas/commands/<file>.json` | Per-command JSON Schema assets. |
| `POST` | `/bridge` | Accept a `CoreCommand` JSON body; return `CoreResponse`. |
| `POST` | `/events/drain` | Drain buffered `CoreEvent`s from the workspace sink. |

## Authentication

When configured with `require_auth_token`, bridge and event routes require header `x-forge-server-token: <token>`.

## Bridge envelope

Request body is a `CoreCommand`:

```json
{
  "request_id": "req-1",
  "actor": { "actor": "operator", "role": "owner" },
  "workspace_id": "ws-1",
  "name": "system.describe",
  "payload": { "tier": "public" }
}
```

Response is a `CoreResponse` with `ok`, `payload`, and optional `error`.

## Tests

```sh
cd forge
cargo test -p forge-server --locked
```

The `console_and_bridge_system_describe_smoke` test covers `/console`, schema serving, `/bridge`, and `/docs`.