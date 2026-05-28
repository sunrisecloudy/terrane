# Fake Platform Host

The fake host is a test double for native hosts. It lets Codex and CI test the MCP server and generated app packages without all native toolchains.

Minimum behavior:

- Accept dev control commands.
- Validate and install webapp packages.
- Simulate the runtime bridge.
- Capture storage, bridge calls, console logs, and event logs.
- Run micro-test steps.
- Return failure bundles.

It does not need to perfectly emulate WKWebView/WebView2/WebKitGTK. Native hosts still need platform smoke tests.

## v0.4 persistence requirement

Implement the platform database layer for this target. Native/fake hosts use SQLite. The server supports SQLite in dev and the Postgres-compatible logical schema in production. The target must run migrations, persist app registry/package/storage/log/test records, and expose safe DB inspection through the dev control plane.
