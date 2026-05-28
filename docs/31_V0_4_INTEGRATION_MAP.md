# v0.4 Integration Map

This document ties together the runtime, generated app package format, persistence/database layer, native shells, Zig core, Codex control plugin, and test system.

## 1. End-to-end lifecycle

```text
AI prompt
  -> generated source package
       manifest.json
       index.html
       styles.css
       app.js
       smoke-tests.json
       migrations/ optional
  -> package validator
       schema checks
       static HTML/CSS/JS policy checks
       permission and capability checks
       network policy checks
       resource-budget checks
       accessibility preflight
  -> package canonicalizer
       normalized paths
       normalized JSON
       content hashes
  -> signer
       signature.json
       install-report.json
  -> platform database transaction
       apps/app_versions/app_files/app_permissions/app_installations
       app_install_reports
       optional app_migrations
  -> app registry
       immutable installed version
       active-version pointer
       previous-version history
  -> runtime mount gate
       load active app version from platform DB
       verify signature
       verify capabilities
       verify user approval
       activate budgets/policies
  -> sandboxed WebView app
       calls AppRuntime.call()
  -> runtime bridge dispatcher
       permission check
       capability check
       resource-budget check
       network policy check
  -> native host or Zig core
       performs effect
  -> result returned to generated app
```

## 2. Core contracts

| Contract | File/spec | Implemented by | Tested by |
|---|---|---|---|
| Generated manifest | `schemas/manifest.schema.json` | validator | schema + package tests |
| Package validation | `docs/04_WEBAPP_PACKAGE_SPEC.md` | runtime tools/fake-host | mutation tests |
| Signing/trust | `docs/17_APP_SIGNING_AND_TRUST.md` | installer/app registry | signing tests |
| Version/rollback | `docs/18_APP_VERSIONING_AND_ROLLBACK.md` | app registry | rollback tests |
| Migrations | `docs/19_DATA_MIGRATIONS.md` | storage/migration runner | migration tests |
| Capabilities | `docs/20_RUNTIME_CAPABILITIES.md` | all platform hosts | capability matrix tests |
| Snapshot/replay | `docs/21_SNAPSHOT_AND_REPLAY_FORMAT.md` | fake-host/dev hosts | replay tests |
| Budgets | `docs/22_RESOURCE_BUDGETS.md` | runtime bridge/sandbox | budget tests |
| Accessibility | `docs/23_ACCESSIBILITY_CONTRACT.md` | runtime/control plane | accessibility tests |
| Network policy | `docs/24_NETWORK_POLICY.md` | network bridge | network mutation tests |
| Codex repair | `docs/25_CODEX_REPAIR_LOOP.md` | MCP + fake-host | micro-tests |
| Database schema | `docs/27_DATABASE_SCHEMA.md` + `db/` | native hosts/server/fake-host | DB tests |
| Storage/migrations | `docs/28_STORAGE_AND_MIGRATIONS.md` | platform storage service | migration + rollback tests |
| Backup/import/export | `docs/29_BACKUP_EXPORT_IMPORT.md` | database service/control plane | export/import tests |

## 3. Platform responsibility split

### Generated webapp

- renders UI inside sandbox;
- calls `AppRuntime.call` only;
- declares permissions/capabilities/policy/budgets;
- has no direct platform authority.

### Runtime WebView layer

- loads app packages;
- owns sandbox and bridge dispatcher;
- enforces manifest permissions;
- enforces resource budgets and network policy;
- exposes capabilities and dev snapshots.

### Native host

- owns real platform effects: file dialogs, storage backend, notifications, WebView lifecycle, native menus, OS integration;
- verifies installed package signatures before mount;
- hosts dev control plane in dev builds only.

### Zig core

- receives deterministic `core.step` events;
- returns deterministic actions;
- stays independent of WebView/platform UI;
- is used by native shells and server.

### Codex plugin/MCP

- never bypasses runtime policy;
- drives the platform through the dev control plane;
- validates, signs, installs, snapshots, micro-tests, repairs, and retests generated apps.

## 4. Implementation dependency graph

Build in this order:

```text
schemas
  -> fake-host package validator
  -> fake-host bridge/runtime mock
  -> Zig core stub
  -> example app validation
  -> signing + install reports
  -> immutable app registry
  -> SQLite/Postgres logical schema
  -> storage bridge implementation
  -> runtime mount gate
  -> resource/network/capability enforcement
  -> snapshot/replay
  -> Codex MCP tools
  -> desktop hosts
  -> mobile hosts
  -> server parity
```

Do not start with native hosts before fake-host behavior is stable. The fake-host defines the reference contract.

## 5. First-version success criteria

A v0.4 implementation is coherent when:

1. all example apps validate against schemas;
2. all example apps install through signing path;
3. all example apps run on fake-host;
4. each native host loads the same runtime and examples;
5. `runtime.capabilities` works on each target;
6. `core.step`, storage, dialogs, network, notifications, logs, snapshots, rollback, and resource audits work through the bridge/control plane;
7. Codex can run one micro-test, find a failure, patch a generated app, reinstall it, and pass the test without bypassing policy.


## 6. Persistence integration rule

All persistent platform state goes through the platform database service. Native hosts use SQLite by default. The server uses SQLite for development and a Postgres-compatible logical schema for production. Generated apps never access SQL, never create tables, and never receive database credentials.

```text
Generated webapp
  -> AppRuntime.call("storage.*")
  -> bridge permission/prefix checks
  -> platform storage service
  -> app_storage(app_id, key, value_json)
```

Codex uses safe control-plane DB inspection tools only:

```text
db.snapshot
db.query_app_storage
db.query_app_versions
db.query_bridge_calls
db.query_core_events
db.query_test_runs
db.export_debug_bundle
```

Arbitrary SQL is disabled by default and may exist only as an explicit unsafe dev-mode tool.
