# Val Town — deployed JS as the app unit

"Write JavaScript, live at a URL in under 100 ms." A *val* is a collaborative,
versioned folder of deployed code. The best checklist of **triggers** — the
ways code gets invoked — and the platform most explicitly built for agents to
build on (Townie assistant + a full MCP server).

## Key features

- **Trigger types:** HTTP endpoints (Request/Response, frameworks like Hono),
  **cron** (intervals or cron syntax, UTC), and **email handlers** — a val
  gets an email address and runs on inbound mail.
- **Storage:** per-val SQLite (since Jan 2026) + blob storage; env vars for
  secrets.
- **Publishing:** every val is instantly a live URL; custom domains; static
  sites.
- **Versioning:** vals are versioned with branches; deploy == save.
- **Agent-first:** the Val Town MCP server can create/edit/run vals, read
  SQLite/blob/log data, configure triggers, manage env vars, view history —
  Townie (their assistant) runs on it. Logs are first-class for self-debugging.
- Deno sandbox: network allowed, filesystem/subprocess restricted.

## What it validated for Terrane

- The trigger taxonomy maps 1:1: HTTP → [../cap-webhook.md](../cap-webhook.md),
  cron → [../cap-scheduler.md](../cap-scheduler.md), storage → kv/relational_db/
  [../cap-blob.md](../cap-blob.md), logs → [../cap-telemetry.md](../cap-telemetry.md),
  Townie-over-MCP → Terrane's harness/builder over the host MCP (already
  shipped).
- Versioned deploys → [../cap-app-update.md](../cap-app-update.md).

## What it exposed

- **Inbound email as a first-class trigger** — promoted receive from non-goal
  to confirmed v2, delivered via interop's `common.receive`
  ([../cap-common.md](../cap-common.md)).
- **Publish-to-web with custom domains** →
  [../cap-web-publish.md](../cap-web-publish.md) (Premium relay, host dials
  out only).
- Their per-val logs + MCP `app_logs` reading pattern directly shaped
  telemetry's "agents fetch logs to self-debug" headline use case.

## Sources

- https://docs.val.town/
- https://www.val.town/features
- https://docs.val.town/vals/cron/
