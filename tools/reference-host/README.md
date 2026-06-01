# Reference Host

The reference host is the local contract implementation for native hosts. It lets Codex and CI test the MCP server and generated app packages without every native toolchain, while still defining the bridge/control-plane behavior other targets must match.

Minimum behavior:

- Accept dev control commands.
- Validate and install webapp packages.
- Simulate the runtime bridge.
- Capture storage, bridge calls, console logs, and event logs.
- Run micro-test steps.
- Run bundled smoke tests either with the fast static checker or, when Chrome is available, in a browser-backed CDP runner.
- Return failure bundles.

It does not replace WKWebView/WebView2/WebKitGTK platform smoke tests. Native hosts still need their own launch and bridge coverage.

## Control token

When launched from `src/server.js`, the reference host generates a fresh 32-byte URL-safe control token and writes it to the documented `control.token` path unless `--token-file` is provided. Tests may pass `controlToken` explicitly, but checked-in plugin config must not contain a shared token.

## Browser-backed smoke tests

`runtime.run_smoke_tests` accepts `runner: "browser"` to execute an installed app's `index.html`, `styles.css`, and `app.js` in headless Chrome. The runner injects the documented `AppRuntime.call` surface, sends bridge calls through the reference-host dispatcher, records bridge calls/errors, and drives the bundled `smoke-tests.json` click/fill/select steps against the real DOM.

Set `TERRANE_CHROME_PATH` when Chrome is not in a standard location. Set `TERRANE_SMOKE_RUNNER=browser` to make browser execution the default, or `TERRANE_SMOKE_RUNNER=auto` to try Chrome and fall back to the static checker when unavailable.

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/reference hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
