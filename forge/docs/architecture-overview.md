# Architecture Overview

This document maps the current `forge/` workspace to the M0a executable spine in `prd-merged/01-core-runtime-prd.md` and `prd-merged/09-roadmap-quality-gates-prd.md`.

## Crate Map

| Crate | Current role |
|---|---|
| `forge-domain` | Shared contracts: stable errors, IDs, command/response/event envelopes, manifests, record envelopes, content hashes, and deterministic `RunRecord` shapes. Pure types plus validation; wasm-clean. |
| `forge-storage` | SQLite substrate: KV storage, records projection, oplog, CRDT chunks/snapshots, schema registry bytes, and run-record persistence. Uses `rusqlite` with WAL/NORMAL pragmas. |
| `forge-crdt` | Thin `CrdtDoc` trait plus Loro-backed record/source documents. Keeps Loro errors behind `CoreError::SyncError`. |
| `forge-schema` | Dynamic schema registry with stable actor-scoped field IDs and additive-only changes. Unknown fields and unknown collections are preserved/tolerated. |
| `forge-policy` | Runtime capability and minimal RBAC gate for `ctx.*` calls. Enforces manifest grants, role run permission, host-call budget, and immediate revocation; workspace/run/platform gates are explicit M0a stubs behind `DecisionContext`. |
| `forge-runtime` | Sandbox and record/replay layer. Runs transpiled JS in QuickJS native on non-wasm targets, injects the single `ctx` host object, enforces resource limits, records host calls, and replays deterministically. |
| `forge-pipeline` | Front-of-spine TypeScript handling: SWC type stripping, canonical code hash, and static policy scan for forbidden constructs before QuickJS execution. |
| `forge-ui` | Planned renderer/diff crate. It is currently a stub with UI fixtures and type declarations elsewhere. |
| `forge-core` | Planned public command/event facade for shells. It is currently a stub. |
| `forge-cli` | Planned M0 harness. It currently prints a placeholder. |
| `forge-testkit` | Planned shared test helpers. It is currently a stub. |

## Spine Data Flow

The M0a spine is:

```text
TypeScript applet
  -> forge-pipeline static scan
  -> SWC type stripping
  -> canonical JS code hash
  -> forge-runtime QuickJS realm
  -> injected ctx host API
  -> forge-policy capability checks
  -> HostBridge effects
  -> SQLite storage / record projection
  -> UI tree emission
  -> RunRecord persistence
  -> deterministic replay
```

The runtime exposes exactly one host object, `ctx`. Applet code cannot import storage, SQLite, platform APIs, or network APIs directly. Effects cross the `HostBridge`, where policy, recorder, and budget accounting meet in `HostContext`.

## Command And Event Contract

`forge-domain` already defines the shell-facing vocabulary:

- `CoreCommand` carries `request_id`, `actor`, `workspace_id`, optional `applet_id`, command name, and JSON payload.
- `CoreResponse` returns `ok`, JSON payload, warnings, and an optional stable `CoreError`.
- `CoreEvent` carries `event_id`, optional `applet_id`, event kind, payload, and logical timestamp.

The full command catalog is still planned in `forge-core`; `prd-merged/01` lists commands such as `workspace.create`, `applet.install`, `runtime.run`, `runtime.replay`, `record.put`, `schema.apply_change`, `permission.request_grant`, and sync/AI/secret commands. The implemented crates already keep the lower-level types and errors stable so the facade can wire them without changing the storage/runtime contracts.

## Two-Layer Security Model

Layer 1 is `forge-pipeline` static scanning. It rejects:

- `eval` and `Function`;
- dynamic imports and M0a static imports;
- raw network globals;
- host globals such as `process` and `require`;
- `globalThis` mutation and prototype pollution paths.

Layer 2 is `forge-runtime`. The QuickJS realm exposes no ambient host globals such as `fetch`, `process`, or `require`. Dynamic code evaluation is poisoned in the realm, resource limits are enforced by interrupt/memory/stack settings, and all host calls route through the policy-checked `ctx` bridge.

Policy checks happen for every host call. M0a implements manifest grants, actor role run permission, host-call budget, and revocation. The remaining SC-10 gates are present as `DecisionContext` seams and default to `AllowAll` until workspace policy, run profile, and platform permission providers exist.

## Persistence And Replay

`forge-storage` owns SQLite persistence:

- `kv` backs `ctx.storage`;
- `records` backs `ctx.db`;
- `oplog`, `crdt_chunks`, and `crdt_snapshots` are the CRDT substrate;
- `runs` stores serialized `RunRecord` values for replay.

`forge-runtime` produces a `RunRecord` containing the input, outcome, code hash, random seed, time start, ordered recorded calls, logs, and permission snapshot. Replay rebuilds policy from the recorded permission snapshot and consumes the recorded call trace. Any extra, missing, or mismatched host call is a determinism error.

## Implemented Vs Planned

Implemented now:

- domain contracts and validation;
- SQLite storage subsets;
- Loro record/source CRDT helpers;
- schema registry evolution;
- policy engine for M0a capabilities;
- QuickJS-native runtime, host bridge, limits, and replay;
- SWC transpile and static policy scan;
- fixture/spec corpora under `forge/fixtures`, `forge/corpus`, and `forge/spec`.

Planned or stubbed:

- `forge-core` command dispatcher;
- `forge-cli` end-to-end harness;
- `forge-ui` renderer/diff engine;
- QuickJS-WASM and JavaScriptCore engines;
- full type-check sidecar;
- full network, secrets, files, LLM, schedule, and platform capability grammar;
- cross-engine conformance runner.

## Useful Entry Points

- `forge/std/forge-std.d.ts` for the current applet API.
- `forge/std/ui-catalog.d.ts` for the broader planned UI-2 catalog.
- `forge/crates/domain/src/manifest.rs` for manifest shape.
- `forge/crates/runtime/src/lib.rs` for the sandbox overview.
- `forge/crates/pipeline/src/lib.rs` for TypeScript and static scanning.
- `forge/crates/policy/src/lib.rs` for capability enforcement.
- `forge/fixtures/e2e` for high-level spine scenarios.
