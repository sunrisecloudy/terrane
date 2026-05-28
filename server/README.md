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
- `POST /bridge` for `core.step`, `runtime.capabilities`, `storage.*`, `app.log`, and structured bridge errors
- `POST /webapps/validate` for server-side package shape and static policy validation
- `GET /webapps/examples`
- `POST /control/command` for token-gated `platform.health`, `runtime.capabilities`, and safe `db.*` inspection commands
- `POST /db/snapshot`, `/db/app-storage`, `/db/app-versions`, `/db/bridge-calls`, `/db/core-events`, `/db/test-runs`, and `/db/export-debug-bundle` for token-gated safe DB inspection

## v0.4 persistence requirement

Server dev persistence uses SQLite through `app_storage(app_id, key, value_json)`, `runtime_sessions`, and `bridge_calls`. `app.log` validates `level`/`message`, writes a redacted `bridge_calls` row, and mirrors the message to stderr. By default the server writes `server-platform.sqlite` in the current working directory; set `NATIVE_AI_SERVER_DB=/path/to/platform.sqlite` to choose another file.

Safe DB inspection endpoints and `/control/command` require `X-Platform-Control-Token` to match `NATIVE_AI_SERVER_CONTROL_TOKEN`. They expose fixed read-only queries only; arbitrary SQL is not available. The current server DB surface includes `app_storage`, `runtime_sessions`, and `bridge_calls`; app version, core event, and test-run inspection endpoints return empty arrays until those records are implemented.

Remaining persistence work: full app registry/package/install/test/control records, migrations, and the Postgres production adapter.
