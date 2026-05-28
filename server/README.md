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
mkdir -p zig-out
zig build-exe -target aarch64-macos.15.0 -lc --dep zig_core -Mroot=src/main.zig -target aarch64-macos.15.0 -Mzig_core=../zig-core/src/lib.zig -femit-bin=zig-out/server
./zig-out/server --port 8088
```

Implemented endpoints:

- `GET /health`
- `POST /core/step`
- `GET /webapps/examples`

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/fake hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
