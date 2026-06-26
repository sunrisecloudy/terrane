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

Derived from `COMMANDS` (`forge/crates/core/src/commands/mod.rs:68`) and roles
from `auth.rs`. **Roles below are indicative** and must be reconciled against
`auth.rs` during Phase 1 — this table is the worksheet, not the final truth.

### workspace.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `workspace.create` | ✅ | operator | Owner |
| `workspace.open` | — | operator | Owner, Maintainer |
| `workspace.export` | — | operator | Owner, Maintainer, Auditor |
| `workspace.import` | ✅ | admin | Owner |

### applet.* (lifecycle)

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `applet.install` | ✅ | operator | Owner, Maintainer |
| `applet.enable` | ✅ | operator | Owner, Maintainer |
| `applet.suspend` | ✅ | operator | Owner, Maintainer |
| `applet.upgrade` | ✅ | operator | Owner, Maintainer |
| `applet.uninstall` | ✅ | operator | Owner, Maintainer |

### runtime.* / ui.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `runtime.run` | ✅ | public | Owner, Maintainer, Editor, Runner |
| `runtime.replay` | — | public | Owner, Maintainer, Editor, Runner |
| `runtime.replay_session` | — | public | Owner, Maintainer, Editor, Runner |
| `ui.dispatch_event` | ✅ | public | Owner, Maintainer, Editor, Runner |

### query.* / db.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `query.execute` | — | public | Owner, Maintainer, Editor, Viewer, Auditor |
| `db.watch` | — | public | (as `query.execute`) + `db.read:<coll>` |
| `db.unwatch` | ✅ | public | (idempotent) |
| `db.history` | — | public | `db.read:<coll>` |
| `db.restore` | ✅ | operator | `db.write:<coll>` |

### schema.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `schema.apply_change` | ✅ | operator | Owner, Maintainer |
| `schema.validate_compatibility` | — | operator | Owner, Maintainer |
| `schema.rebuild_indexes` | ✅ | operator | Owner, Maintainer |

### sync.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `sync.trust_peer` | ✅ | admin | Owner |
| `sync.export` | — | operator | Owner, Maintainer |
| `sync.import` | ✅ | operator | Owner, Maintainer (per-chunk membership check) |

### quota.* / audit.*

| Command | Mutates | Vis | Indicative roles |
| --- | --- | --- | --- |
| `quota.status` | — | operator | Owner, Maintainer, Auditor |
| `quota.set` | ✅ | admin | Owner |
| `quota.auto_quarantine` | ✅ | admin | Owner |
| `audit.query` | — | admin | Owner, Auditor (oversight) |

### package.* (legacy webapp compatibility)

| Command | Mutates | Vis | Notes |
| --- | --- | --- | --- |
| `package.get_manifest` | — | operator | legacy stability |
| `package.get_permissions` | — | operator | legacy |
| `package.provision_registry` | ✅ | admin | legacy |
| `package.list_versions` | — | operator | legacy |
| `package.activate_version` | ✅ | operator | legacy |
| `package.rollback_version` | ✅ | operator | legacy |
| `package.set_status` | ✅ | admin | legacy |

### bridge.* (Phase C gates) + legacy

| Command | Mutates | Vis | Notes |
| --- | --- | --- | --- |
| `bridge.validate_network_request` | — | debug | internal gate |
| `bridge.validate_envelope` | — | debug | internal gate |
| `bridge.prepare_session` | ✅ | debug | internal |
| `bridge.record_call` | ✅ | debug | internal |
| `bridge.record_core_event` | ✅ | debug | internal |
| `bridge.record_crash_recovery` | ✅ | debug | internal |
| `legacy.core_step` | ✅ | debug | v0.4 compat, time-limited |

### control.* (feature `control`, debug-only)

`control.compare_snapshot`, `control.json_matches_subset`,
`control.package_validate`, `control.package_hashes`, `control.backup_validate`,
`control.backup_content_hash`, `control.generate_token`, `control.sign_payload`,
`control.verify_signature` — all `visibility: debug`, only present under the
`control` feature. Note these intersect the retired `/control` decision; treat as
**debug-gated, not part of the public CLI surface** unless explicitly revived.

## Inner surface (`ctx.*`) — reference entries

Cataloged for documentation and capability reasoning only; `surface: "inner"`,
never targetable by `forge run`:

`ctx.db`, `ctx.net`, `ctx.files`, `ctx.ui`, `ctx.secrets`, `ctx.timetravel`,
`ctx.future` (and the lower-level `db.*`, `files.*`, `net.fetch`,
`network.egress`, `random.next` host-calls these resolve to). Enumerated in the
runtime/host specs and `generatedAppBoundary.api` of the public contract
(`tools/export-public-contract.mjs:213`).

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
