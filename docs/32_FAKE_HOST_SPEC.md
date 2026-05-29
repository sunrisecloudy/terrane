# Fake Host Spec

## 1. Purpose

The fake host is the **reference implementation** of the bridge contract. Every native shell and the server are diffed against it; if the fake host says a method returns `{"value":[]}`, every other host must return the same bytes for the same input.

The fake host lives at `tools/fake-platform-host/` and runs as a Node.js process (no native bundle). It serves:

- the WebView runtime (`runtime-web/`);
- the bundled example apps (`webapps/examples/`);
- the bridge contract (`AppRuntime.call`);
- the dev control plane (`/control/*`);
- the platform database (SQLite, in-memory by default).

Build-free apps run inside a browser tab pointed at the fake host. Codex micro-tests run against the fake host first; only after a green run on the fake host does Codex run platform-smoke tests on real native hosts.

## 2. Why a separate doc

Previous revisions referenced "the fake host" across docs 14, 15, 17, 19, 27, and 31 without defining it. This document is the single normative source for what the fake host must do and must not do.

## 3. Process model

```text
node tools/fake-platform-host
  ↓
HTTP server on 127.0.0.1:7878 (configurable via --port)
  ↓
serves:
  GET  /                         -> runtime launcher (index.html from runtime-web/)
  GET  /runtime/*                -> runtime assets (from runtime-web/)
  GET  /webapps/examples/*       -> example app packages
  POST /bridge                   -> bridge dispatch (docs/03)
  POST /control/sessions         -> dev control plane (docs/14)
  ...                            -> the full control-plane surface
  GET  /health                   -> { ok: true, version, db: "sqlite-mem" }
```

The fake host does not bind any non-loopback address by default. `--bind 0.0.0.0` is a dev-only flag; it logs a loud warning and refuses to run when the dev token file is missing.

## 4. Storage

### 4.1 Default mode

- SQLite in-memory database.
- All `db/sqlite/*.sql` migrations applied at startup.
- Programmatic test startup begins with empty `apps`, `app_versions`, etc. The CLI dev server preloads bundled apps by default so the visual launcher exercises installed packages.

### 4.2 File-backed mode

- `--db-file <path>` opens or creates a SQLite file at that path.
- The file is created with mode `0600`.
- On open, the host runs `PRAGMA integrity_check` and refuses to start if it fails.

### 4.3 Reset

`DELETE /control/db` (token-protected) drops and re-applies all migrations. Used by micro-test teardown.

## 5. Bridge dispatch behavior

The fake host implements every method in docs/03 §3:

- `core.step` calls into a JS-bound Zig core via WebAssembly (`zig build wasm`) or a Node addon. The fake host must be able to load the same `libzig_core` deterministically.
- `storage.*` reads/writes `app_storage` rows directly.
- `dialog.openFile` / `dialog.saveFile` return values from a mock registry (`runtime.dialog_mock_set`) — there is no real file picker.
- `notification.toast` is captured in an in-memory queue accessible via `runtime.notification_capture`.
- `network.request` is served by the mock registry (`runtime.network_mock_set`). Direct network is **forbidden** by default; opting in requires `--allow-real-network` and is intended only for diagnosing host-only test failures.
- `app.log` writes to `runtime_sessions` and to stderr.
- `runtime.capabilities` returns the fake-host capability document.

## 6. Capability document

```json
{
  "runtimeVersion": "0.1.0",
  "platform": "fake",
  "target": "fake-host",
  "devMode": true,
  "features": {
    "core.step": true,
    "storage.read": true,
    "storage.write": true,
    "dialog.openFile": true,
    "dialog.saveFile": true,
    "network.request": true,
    "notification.toast": true,
    "app.log": true,
    "runtime.capabilities": true,
    "runtime.snapshot": true,
    "runtime.replay": true
  },
  "limits": {
    "maxBodyBytes": 1048576,
    "maxStorageBytes": 5242880,
    "maxBridgeCallsPerMinute": 600,
    "maxPackageBytes": 4194304,
    "maxFileBytes": 2097152
  }
}
```

Fake host features that use mock registries (`network.request`, `dialog.openFile`, and `dialog.saveFile`) report `true` in the capability document. Calls that have no matching mock return `network.mock_missing` / `dialog.mock_missing`.

## 7. Control plane

The fake host implements the full control plane in docs/14 with one difference: it accepts `algorithm = "none-dev"` signatures and `devUnsigned: true` installs (docs/17 §10). All other invariants — token auth, `127.0.0.1` bind, audit log to `control_commands` — apply.

## 8. Reference contract guarantees

The fake host must satisfy these guarantees, every other host must match:

1. **Byte-identical bridge responses for the same input.** For every contract fixture under `tests/fixtures/bridge/`, the fake host's response must equal the native host's response after stripping (`id`, `timestamp`) fields, except for fields explicitly covered by a fixture's `expectedByPlatform` entry for platform identity or stricter pre-mount rejection.
2. **Same error codes for the same error conditions.** Codes are listed in docs/03 §5.
3. **Same `app_install_reports` shape after installing the same package.** Hashes (`manifestHash`, `contentHash`) must be identical because canonicalization (docs/17 §6) is deterministic.
4. **Same `core.step` actions for the same event stream.** Determinism is enforced by Zig core; the fake host loads the same compiled Zig.
5. **Same snapshot/replay output for the same inputs.** Snapshot schema validation passes everywhere.

Drift between the fake host and any native host is a bug in the native host unless the fake host is itself non-conformant; in that case it is a bug in the fake host. The fake host is never "behind" — it must be at least at the latest spec revision.

## 9. CLI

```text
node tools/fake-platform-host [options]

Options:
  --port <n>               default 7878
  --bind <addr>            default 127.0.0.1
  --db-file <path>         use file-backed SQLite instead of :memory:
  --seed-bundled           preload bundled example apps with trustLevel="bundled" (CLI default)
  --no-seed-bundled        start the CLI with an empty app registry
  --allow-runtime-mismatch dev override for runtime version compat
  --allow-real-network     allow network.request to perform real fetches (dangerous)
  --token-file <path>      where to write the per-launch control token
  --log-level <level>      one of debug|info|warn|error (default info)
```

## 10. Non-goals

- Not a production server. Use the Zig server (`server/`) for production parity.
- Not a substitute for native host smoke tests. Platforms with a real WKWebView / WebView / WebView2 / WebKitGTK may surface bugs the fake host cannot.
- Not a UI framework. The fake host serves whatever runtime-web produces; it does not implement a launcher of its own.

## 11. Test obligations

The fake host has its own contract tests under `tools/fake-platform-host/test/`. CI runs them on every PR. A PR that breaks the fake host blocks the merge because every other host is diffed against it.

## 12. Relationship to `__APP_RUNTIME_DEV_MOCK__`

The browser-only mock host (`window.__APP_RUNTIME_DEV_MOCK__ = true`, docs/03 §7) is a *separate* fast-loop dev convenience for editing the runtime in a browser tab without a Node process. It is not the reference contract. When in doubt, defer to the fake host.
