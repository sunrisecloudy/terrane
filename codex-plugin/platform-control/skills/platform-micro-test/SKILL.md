---
name: platform-micro-test
description: Use this when Codex must launch or attach to the Native AI Webapp Platform, install generated webapps, inspect UI/runtime/bridge/core state, and run granular micro-tests.
---

# Platform micro-test skill

Use the platform-control MCP tools to test generated webapps and runtime behavior.

## Workflow

1. Call `platform.health`.
2. If no host is attached, call `platform.launch` with the requested target, defaulting to `fake-host` for fast tests.
3. Validate the webapp package before installing it.
4. Install the package.
5. Open the app.
6. Wait for runtime idle.
7. Run declared smoke tests and relevant `.microtest.json` tests.
8. On failure, collect screenshot, DOM snapshot, runtime snapshot, console logs, bridge logs, storage snapshot, and core event log.
9. Report whether the failure belongs to generated app code, runtime code, bridge/native code, or Zig core.

## Rules

- Prefer `data-testid` selectors.
- Do not use arbitrary unsafe evaluation unless the user explicitly asks for it and dev mode enables it.
- Do not bypass manifest permissions.
- Do not call bridge methods not documented in `docs/03_RUNTIME_API_SPEC.md`.
- Destructive operations require `confirm: true`.

## Database-level assertions

When a UI/bridge assertion is ambiguous, use safe DB tools to inspect persisted state:

- `db.query_app_storage`
- `db.query_bridge_calls`
- `db.query_core_events`
- `db.query_test_runs`

Do not request arbitrary SQL. Prefer exact app id, key prefix, and session id filters.
