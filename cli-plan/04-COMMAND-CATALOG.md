# 04 — The Command Catalog (the keystone)

This is the single most important artifact in the plan. Everything else is a
projection of it. This file defines **the descriptor model** and **a full
enumeration** of today's commands with the metadata to attach.

## The descriptor model

Each command gains a descriptor co-located with its registry entry. Conceptual
shape (final Rust form decided in [05](05-PHASE-1-SELF-DESCRIBING-REGISTRY.md)):

```jsonc
{
  "name": "applet.install",            // unique, namespaced; matches COMMANDS key
  "namespace": "applet",               // derived from name prefix
  "summary": "Install an applet from a manifest + sources.",
  "surface": "outer",                  // "outer" (operator) | "inner" (ctx.* app call)
  "mutates": true,                     // changes durable state / emits domain events?
  "effectful": false,                  // touches host effects (net/files/clock/random)?
  "visibility": "operator",            // public | operator | admin | debug
  "required_roles": ["Owner", "Maintainer"],   // derived from authorize() — F3
  "capabilities": [],                  // e.g. ["db.write:<collection>"] secondary gates
  "payload_schema": "schemas/commands/applet.install.request.schema.json",
  "response_schema": "schemas/commands/applet.install.response.schema.json",
  "events": ["applet.installed"],      // CoreEvents this may emit
  "stability": "stable",               // stable | preview | legacy | deprecated
  "since": "m0a",                      // first milestone it appeared
  "examples": [ { "payload": { /* ... */ } } ]
}
```

### Field rules

| Field | Source of truth | Rule |
| --- | --- | --- |
| `name` | the `COMMANDS` key | must equal the dispatch key exactly; uniqueness enforced |
| `namespace` | derived | the prefix before the first `.` |
| `summary` | descriptor | one line, imperative; required |
| `surface` | descriptor | `inner` only for `ctx.*` reference entries; `run` rejects `inner` |
| `mutates` | descriptor, test-checked | read-only commands must be `false`; used by console to warn |
| `effectful` | policy (`forge-policy`) | drives the "this touches the network/disk" warning + replay notes |
| `visibility` | descriptor | gates each front-end (see [10](10-SECURITY-AND-RBAC.md)) |
| `required_roles` | `auth.rs` table (F3) | **derived**, never hand-copied; a mismatch is a test failure |
| `capabilities` | `auth.rs` capability gates | secondary, collection-scoped grants |
| `payload_schema` / `response_schema` | `schemas/` / `forge-schema` | reference existing where possible; new files under `schemas/commands/` |
| `events` | handler / `forge-domain` | the `CoreEvent` names the command can emit |
| `stability` / `since` | descriptor | lifecycle bookkeeping; `legacy`/`deprecated` hidden by default |

### Invariants enforced by tests (Phase 1 exit)

1. Every entry in `COMMANDS` (and feature-gated tables) has a descriptor.
2. Every descriptor's `name` resolves to a real handler.
3. `required_roles` in the descriptor equals what `authorize()` enforces (one
   shared table, or a test that cross-checks them).
4. Every `payload_schema` / `response_schema` path exists and parses.
5. `visibility: public` commands never require a privileged-only role (catch
   accidental exposure).
6. `system.describe` round-trips: serializing then validating the catalog is
   stable and deterministic (replay-safe).

## Today's outer commands (to be cataloged)

**42 commands** in `COMMANDS` (`forge/crates/core/src/commands/mod.rs:68`–`:146`)
plus **9** feature-gated `control.*` entries (`:150`–`:187`). Roles below are
**verified against `auth.rs`** (2026-06-26); Phase 1 must derive descriptors
from the unified role table, not re-type these by hand.

### workspace.*

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `workspace.create` | ✅ | operator | Owner |
| `workspace.open` | — | operator | Owner, Maintainer, Editor, Viewer, Auditor |
| `workspace.export` | — | operator | Owner, Maintainer, Auditor |
| `workspace.import` | ✅ | admin | Owner |

### applet.* (lifecycle)

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `applet.install` | ✅ | operator | Owner, Maintainer |
| `applet.enable` | ✅ | operator | Owner, Maintainer |
| `applet.suspend` | ✅ | operator | Owner, Maintainer |
| `applet.upgrade` | ✅ | operator | Owner, Maintainer |
| `applet.uninstall` | ✅ | operator | Owner, Maintainer |

### runtime.* / ui.*

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `runtime.run` | ✅ | public | Owner, Maintainer, Editor, Runner |
| `runtime.replay` | — | operator | Owner, Maintainer, Auditor |
| `runtime.replay_session` | — | operator | Owner, Maintainer, Auditor |
| `ui.dispatch_event` | ✅ | public | Owner, Maintainer, Editor, Runner |

> **Visibility note:** `runtime.replay*` is oversight/audit (not run-capable
> roles). Tier `operator`, not `public`, satisfies the Phase-1 invariant that
> `public` commands are reachable by the broad read/run membership.

### query.* / db.*

| Command | Mutates | Vis | Roles (`auth.rs`) + capabilities |
| --- | --- | --- | --- |
| `query.execute` | — | public | Owner, Maintainer, Editor, Viewer, Auditor + `db.read:<coll>` |
| `db.watch` | — | public | same roles as `query.execute` + `db.read:<coll>` |
| `db.unwatch` | ✅ | public | same roles as `query.execute` |
| `db.history` | — | public | same roles as `query.execute` + `db.read:<coll>` |
| `db.restore` | ✅ | operator | Owner, Maintainer, Editor + `db.write:<coll>` |

### schema.*

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `schema.apply_change` | ✅ | operator | Owner, Maintainer |
| `schema.validate_compatibility` | — | operator | Owner, Maintainer, Editor, Auditor |
| `schema.rebuild_indexes` | ✅ | operator | Owner, Maintainer |

### sync.*

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `sync.trust_peer` | ✅ | admin | Owner |
| `sync.export` | — | operator | Owner, Maintainer, Auditor |
| `sync.import` | ✅ | operator | Owner, Maintainer (per-chunk membership check) |

### quota.* / audit.*

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `quota.status` | — | operator | Owner, Maintainer, Editor, Viewer, Auditor |
| `quota.set` | ✅ | admin | Owner |
| `quota.auto_quarantine` | ✅ | debug | Owner, Maintainer, Editor, Runner (host-runtime group) |
| `audit.query` | — | admin | Owner, Maintainer, Auditor |

### package.* (legacy webapp compatibility)

All seven share the **run-capable host-runtime role set** in `auth.rs:63`–`:76`
(same as `legacy.core_step` / `bridge.*`). Visibility `debug` — shell-internal,
not operator CLI surface by default.

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `package.get_manifest` | — | debug | Owner, Maintainer, Editor, Runner |
| `package.get_permissions` | — | debug | Owner, Maintainer, Editor, Runner |
| `package.provision_registry` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `package.list_versions` | — | debug | Owner, Maintainer, Editor, Runner |
| `package.activate_version` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `package.rollback_version` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `package.set_status` | ✅ | debug | Owner, Maintainer, Editor, Runner |

### bridge.* (Phase C gates) + legacy

Host-runtime group (`auth.rs:63`–`:76`): Owner, Maintainer, Editor, Runner.
Visibility `debug` — internal shell gates, not default CLI/console surface.

| Command | Mutates | Vis | Roles (`auth.rs`) |
| --- | --- | --- | --- |
| `bridge.validate_network_request` | — | debug | Owner, Maintainer, Editor, Runner |
| `bridge.validate_envelope` | — | debug | Owner, Maintainer, Editor, Runner |
| `bridge.prepare_session` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `bridge.record_call` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `bridge.record_core_event` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `bridge.record_crash_recovery` | ✅ | debug | Owner, Maintainer, Editor, Runner |
| `legacy.core_step` | ✅ | debug | Owner, Maintainer, Editor, Runner |

### control.* (feature `control`, debug-only)

`control.compare_snapshot`, `control.json_matches_subset`,
`control.package_validate`, `control.package_hashes`, `control.backup_validate`,
`control.backup_content_hash`, `control.generate_token`, `control.sign_payload`,
`control.verify_signature` — all `visibility: debug`, only present under the
`control` feature. Note these intersect the retired `/control` decision; treat as
**debug-gated, not part of the public CLI surface** unless explicitly revived.

## Inner surface (`ctx.*`) — reference entries

Cataloged for documentation and capability reasoning only; `surface: "inner"`,
never targetable by `forge run`. These map to the `HostBridge` methods
(`forge/crates/runtime/src/bridge.rs`) and are already recorded per run as
`RecordedCall`/`RunRecord` (`forge/crates/domain/src/run.rs:49`); see
[14-EFFECT-SURFACE-AND-OBSERVABILITY.md](14-EFFECT-SURFACE-AND-OBSERVABILITY.md)
for the two-door decision and how the journal is exposed via `system.trace`.

`ctx.db`, `ctx.net`, `ctx.files`, `ctx.ui`, `ctx.secrets`, `ctx.timetravel`,
`ctx.future` (and the lower-level `HostBridge` methods in
`forge/crates/runtime/src/bridge.rs:31`–`:196` — `storage_*`, `db_*`, `ui_render`,
`net_fetch`, `files_write`, `secret_store`, `log`, etc.). The public contract
lists the high-level `ctx.*` namespaces at
`tools/export-public-contract.mjs:229` (`generatedAppBoundary.api`).

### Contract export drift (today)

The hand-maintained `CORE_COMMANDS` array in `export-public-contract.mjs:86`–`:121`
is **out of sync** with `COMMANDS` — see [01-FINDINGS.md](01-FINDINGS.md) F11.
Phase 11 replaces that list with the emitted catalog so export, registry, and
`forge/spec/commands.md` converge.

## Where the catalog data physically lives

Decided in detail in [05](05-PHASE-1-SELF-DESCRIBING-REGISTRY.md) and
[11](11-SCHEMAS-AND-CONTRACT.md); the shortlist:

- **Descriptors:** a Rust table beside `COMMANDS`, or a checked-in
  `forge/data/commands.json` loaded at build/startup (mirrors the existing
  `forge/data/*.json` extraction pattern used by `forge-core-plan`).
- **Schemas:** new `schemas/commands/<name>.request|response.schema.json`,
  reusing existing object schemas by `$ref` where shapes already exist.
- **Roles:** the existing `auth.rs` table, refactored so both `authorize()` and
  the catalog read one source.
