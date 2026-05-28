# Fake Platform Host

The fake host is a test double for native hosts. It lets Codex and CI test the MCP server and generated app packages without all native toolchains.

Minimum behavior:

- Accept dev control commands.
- Validate and install webapp packages.
- Simulate the runtime bridge.
- Capture storage, bridge calls, console logs, and event logs.
- Run micro-test steps.
- Run bundled smoke tests either with the fast static checker or, when Chrome is available, in a browser-backed CDP runner.
- Return failure bundles.

It does not need to perfectly emulate WKWebView/WebView2/WebKitGTK. Native hosts still need platform smoke tests.

## Browser-backed smoke tests

`runtime.run_smoke_tests` accepts `runner: "browser"` to execute an installed app's `index.html`, `styles.css`, and `app.js` in headless Chrome. The runner injects the documented `AppRuntime.call` surface, sends bridge calls through the fake-host dispatcher, records bridge calls/errors, and drives the bundled `smoke-tests.json` click/fill/select steps against the real DOM.

Set `NATIVE_AI_CHROME_PATH` when Chrome is not in a standard location. Set `NATIVE_AI_SMOKE_RUNNER=browser` to make browser execution the default, or `NATIVE_AI_SMOKE_RUNNER=auto` to try Chrome and fall back to the static checker when unavailable.

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/fake hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
