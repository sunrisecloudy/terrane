# Terrane — Implementation Spec v0.4

This package is an implementation-ready specification for a native app platform that can generate and run build-free webapps on the fly, while sharing deterministic core logic through Zig and persisting platform state in a portable SQLite/Postgres-compatible database layer.

```text
Native host apps
  iOS/macOS: Swift + WKWebView (WKScriptMessageHandlerWithReply)
  Android: Kotlin + Android WebView (WebViewCompat.addWebMessageListener)
  Windows: C++/WinRT + WebView2 (WebMessageReceived)
  Linux: C/GTK4 + WebKitGTK
  Server: Zig executable

Reference contract
  Reference host (Node + SQLite) under tools/reference-host
  Every native host is diffed against the reference host byte-for-byte

Web runtime
  Build-free HTML/CSS/vanilla JS, loaded inside each host WebView
  Per-mount MessageChannel issues a mount_token; appId is derived, not declared
  Sandboxed generated webapps with strict CSP + Trusted Types where supported
  Permission/capability/budget checks

Core logic
  Zig library with a coarse byte/message API
  Deterministic event -> action state machine

Persistence
  SQLite on every native host and the reference host
  Postgres-compatible logical schema on the server in production
  Generated apps see only storage.* — SQL never crosses the boundary
```

## Where to start

| Reader | Start here |
|---|---|
| New to the project | `docs/00_PRD.md` |
| Implementing a host | `docs/01_ARCHITECTURE.md` → `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md` → `docs/32_REFERENCE_HOST_SPEC.md` |
| Writing or repairing a webapp | `docs/03_RUNTIME_API_SPEC.md` → `docs/04_WEBAPP_PACKAGE_SPEC.md` → `docs/15_MICRO_TESTING_PROTOCOL.md` |
| Working on Codex MCP plugin | `docs/14_CODEX_CONTROL_PLUGIN.md` → `docs/16_CODEX_PLUGIN_IMPLEMENTATION_PLAN.md` |
| Reviewing what's built | `IMPLEMENTATION_STATUS.md` |

## What is inside

- `docs/00_PRD.md` — product requirements document with feature → milestone matrix.
- `docs/01_ARCHITECTURE.md` — full architecture and data flow.
- `docs/02_PROJECT_STRUCTURE.md` — recommended monorepo tree.
- `docs/03_RUNTIME_API_SPEC.md` — runtime, bridge, and app APIs, including the channel-derived `appId` rule.
- `docs/04_WEBAPP_PACKAGE_SPEC.md` — generated webapp package format and runtime version compatibility.
- `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md` — iOS, macOS, Android, Windows, Linux, and server requirements.
- `docs/06_ZIG_CORE_SPEC.md` — Zig core library contract and FFI surface.
- `docs/07_SECURITY_MODEL.md` — sandbox, permissions, CSP + Trusted Types, app-id derivation, audit logging.
- `docs/08_TEST_PLAN.md` — all-level test plan with the bridge contract fixture format.
- `docs/09_CODEX_IMPLEMENTATION_PLAN.md` — milestones and Codex-ready implementation order (the reference host is Milestone 2).
- `docs/10_ACCEPTANCE_CHECKLIST.md` — first-version acceptance criteria.
- `docs/11_AI_GENERATION_PROMPTS.md` — prompts for generating webapps on the fly.
- `docs/12_RELEASE_AND_CI.md` — build, packaging, CI plan, and runtime self-update.
- `docs/13_EXAMPLE_APP_COVERAGE.md` — example app coverage matrix.
- `docs/14_CODEX_CONTROL_PLUGIN.md` — Codex plugin + MCP control-plane design, including token auth.
- `docs/15_MICRO_TESTING_PROTOCOL.md` — granular app/UI/core test protocol with smoke vs micro relationship.
- `docs/16_CODEX_PLUGIN_IMPLEMENTATION_PLAN.md` — implementation milestones for the plugin and control plane.
- `docs/17_APP_SIGNING_AND_TRUST.md` — Ed25519 signing, per-host keypair, mount-time verification.
- `docs/18_APP_VERSIONING_AND_ROLLBACK.md` — immutable versions, rollback, quarantine.
- `docs/19_DATA_MIGRATIONS.md` — declarative migration grammar and pipeline.
- `docs/20_RUNTIME_CAPABILITIES.md` — capability negotiation.
- `docs/21_SNAPSHOT_AND_REPLAY_FORMAT.md` — deterministic snapshots.
- `docs/22_RESOURCE_BUDGETS.md` — budgets, platform clamps, and performance methodology.
- `docs/23_ACCESSIBILITY_CONTRACT.md` — accessibility requirements.
- `docs/24_NETWORK_POLICY.md` — host-mediated network request policy (`networkPolicy`, replaces `networkAllowlist`).
- `docs/25_CODEX_REPAIR_LOOP.md` — generate → validate → sign → install → micro-test → repair workflow.
- `docs/26_PLATFORM_CAPABILITY_MATRIX.md` — required platform support matrix.
- `docs/27_DATABASE_SCHEMA.md` — SQLite/Postgres logical database model.
- `docs/28_STORAGE_AND_MIGRATIONS.md` — generated app storage bridge and migrations.
- `docs/29_BACKUP_EXPORT_IMPORT.md` — portable backup/debug bundle export and import.
- `docs/30_DATABASE_TEST_PLAN.md` — schema, storage, rollback, migration, logging, export/import tests.
- `docs/31_V0_4_INTEGRATION_MAP.md` — integrated runtime/database/Codex/native/Zig lifecycle.
- `docs/32_REFERENCE_HOST_SPEC.md` — reference host as the reference contract.
- `IMPLEMENTATION_STATUS.md` — single source of truth for what's built vs planned.
- `AGENTS.md` — Codex working agreements (hard rules).
- `codex-plugin/platform-control/` — local Codex plugin skeleton with skills and MCP config.
- `tools/codex-platform-mcp/` — MCP server contract/starter files.
- `tools/reference-host/` — reference host (Milestone 2 deliverable).
- `devtools/control-plane/` — developer-only host control-plane contract (OpenAPI).
- `schemas/` — JSON Schemas for manifests, bridge calls, packages, core messages, signatures, migrations, capabilities, snapshots, budgets, network policy, install reports, accessibility reports, and DB records.
- `db/sqlite/` and `db/postgres/` — platform database migrations.
- `webapps/examples/` — five canonical build-free webapp packages.
- `codex/` — master prompts and guardrails for using Codex to implement the system.

## MVP definition

The platform proves the contract end-to-end when:

1. The reference host runs every example app, every micro-test, and every contract fixture green.
2. Each native host loads the same runtime and examples.
3. Each native host produces byte-identical bridge responses to the reference host (after stripping non-deterministic fields).
4. `runtime.capabilities` works on each target.
5. `core.step`, storage, dialogs, network, notifications, logs, snapshots, rollback, and resource audits work through the bridge/control plane.
6. Codex can run one micro-test, find a failure, patch a generated app, reinstall it, and pass the test without bypassing policy.
7. All persistent state lives in the platform DB; backup export/import round-trips one example app on the reference host without data loss.

## Design goal

Generated apps are treated as content/plugins, not as trusted native code. The runtime is the product. AI generates HTML/CSS/JS packages that run against a fixed, reviewed runtime API.

## Versioning summary

- v0.1 — host shells, Zig core, fixed bridge, 5 examples.
- v0.2 — Codex control plugin + dev control plane.
- v0.3 — signing, immutable installs, rollback, capabilities, budgets, network policy, accessibility, snapshots.
- v0.4 — platform database + transactional installs + backup/export/import.

Detailed feature → milestone matrix lives in `docs/00_PRD.md §0.2`.

## License

Apache License 2.0. See `LICENSE`.
