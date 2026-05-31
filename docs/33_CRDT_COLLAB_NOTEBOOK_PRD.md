# PRD: CRDT Collaborative Notebook and Zig Sync Core

## 0. Document status

This document defines the next product slice after the v0.4 platform baseline: a platform-owned CRDT layer for realtime collaborative notebooks between humans and AI agents.

The feature is intentionally specified as a standalone Zig package first, then integrated into the existing runtime bridge, database, server, native hosts, and reference-host contracts. Generated apps still remain build-free HTML/CSS/vanilla JavaScript and must not import CRDT libraries directly.

## 1. Product summary

The platform must support realtime collaborative notebooks where multiple human users and AI agents can edit, propose, review, and synchronize notebook state across clients.

The notebook state must be:

- conflict-free under concurrent edits;
- durable in the platform database;
- replayable for deterministic debugging;
- permission-checked before every applied operation;
- schema-validated before and after merge;
- portable across native hosts and server;
- testable as a standalone Zig package without WebView or host dependencies.

## 2. Reference library

The reference library for CRDT behavior is Loro.

Loro was selected for the notebook profile because it directly supports document-like collaborative state, text, maps, lists, movable/tree-shaped structures, version history, time travel, and undo/redo-style workflows. These capabilities match notebook cells, rich text, code cells, AI proposals, branching review, and rollback better than plain key/value storage.

The local reference checkout lives at:

```text
external-lib/loro
```

Initial pinned source:

```text
remote: https://github.com/loro-dev/loro
commit: ab91df67e322d01f75621742ff83d0fb4a000e79
```

Loro is a reference oracle and research fixture source for this project. Generated apps must not load Loro, Rust, WebAssembly, npm packages, or direct sync clients unless a future runtime capability explicitly allows that.

## 3. Goals

### G1. Standalone Zig CRDT package

Create a new standalone Zig package, tentatively:

```text
zig-crdt/
```

The package must build and test independently from the host runtime:

```text
cd zig-crdt
zig build test
```

The package owns deterministic CRDT logic only. It must not open sockets, access SQLite/Postgres, inspect WebViews, perform user authentication, or call native platform APIs.

### G2. Loro-backed conformance fixtures

Build a fixture generator that uses the pinned Loro checkout to produce canonical operation streams and expected materialized notebook states.

The Zig implementation must pass differential tests against those fixtures:

- same final materialized JSON;
- same rejected invalid operations;
- same convergence after reordered concurrent operations;
- same stable version/frontier representation for supported profile features;
- same snapshot/import/export behavior for supported update format.

The fixture set must include human-human, human-AI, offline, duplicate-op, out-of-order, and permission-denied scenarios.

### G3. Notebook CRDT profile

The first supported profile is a structured notebook, not an arbitrary CRDT engine.

Notebook root:

```json
{
  "metadata": {},
  "cells": [],
  "comments": {},
  "aiRuns": {},
  "proposals": {},
  "approvals": {}
}
```

Cell shape:

```json
{
  "id": "cell_...",
  "type": "markdown | code | output | artifact | prompt",
  "source": "collaborative text",
  "metadata": {},
  "outputs": [],
  "createdBy": "actor_...",
  "updatedBy": "actor_..."
}
```

The profile must support:

- ordered cell insert/delete/move;
- collaborative text edits inside markdown, prompt, and code cells;
- metadata map set/delete;
- append-only output records;
- comments and comment resolution;
- AI proposal creation and review;
- checkpoints/frontiers for review and rollback.

Presence, cursors, typing indicators, and AI streaming tokens are realtime transport state, not durable CRDT history until committed.

### G4. AI as constrained actor

AI must be modeled as an actor with explicit capabilities, not as a privileged bypass.

Default AI behavior:

- AI may create proposals.
- AI may stream ephemeral draft text.
- AI may not mutate canonical notebook state unless its actor role has direct write permission or a human accepts the proposal.
- AI operations must record model id, prompt/context hash, actor id, affected cell ids, and base frontier.

Human approval state is part of the notebook CRDT profile so review decisions synchronize across clients.

### G5. Runtime bridge surface

Expose notebook CRDT operations only through platform bridge methods. Candidate methods:

```text
notebook.open
notebook.apply_local
notebook.propose_ai_patch
notebook.accept_proposal
notebook.reject_proposal
notebook.snapshot
notebook.checkout
notebook.sync_pull
notebook.sync_push
notebook.subscribe
```

The runtime derives app id, notebook id access, actor identity, and mount/session context. Generated apps must not send `appId`.

Bridge responses must use the existing structured response convention:

```json
{
  "id": "request-id",
  "ok": true,
  "result": {}
}
```

Errors must use existing platform error codes where possible, with new CRDT-specific codes added only when necessary:

- `permission_denied`
- `invalid_request`
- `schema_error`
- `conflict_rejected`
- `stale_frontier`
- `unknown_notebook`
- `sync_unavailable`

### G6. Manifest permissions and capabilities

Add explicit permissions/capabilities before exposing bridge methods:

```json
{
  "permissions": [
    "notebook.read",
    "notebook.write",
    "notebook.propose",
    "notebook.approve",
    "notebook.sync"
  ],
  "capabilities": {
    "required": ["notebook.read"],
    "optional": ["notebook.sync", "notebook.propose"]
  }
}
```

Native/server bridges must re-check every operation using the derived app id, install permissions, notebook ACL, actor role, and AI policy. Runtime checks are not sufficient.

### G7. Platform database persistence

CRDT persistence is platform-owned. Generated apps never access SQL and never store notebook CRDT internals through `localStorage`, IndexedDB, cookies, or direct fetch.

Add SQLite and Postgres logical tables after a schema PRD update:

```text
crdt_notebooks
crdt_documents
crdt_updates
crdt_heads
crdt_actors
crdt_permissions
crdt_proposals
crdt_sync_cursors
```

Storage rules:

- `app_id` is always derived from sandbox/control context.
- Every update belongs to one app and one notebook.
- Updates are append-only unless compacted into a verified snapshot.
- Compaction must preserve convergence and replay guarantees.
- Destructive compaction, migration, rollback, or import requires a snapshot first.

### G8. C ABI and host integration

The Zig package must expose a small C ABI similar to `zig-core`:

```c
typedef struct ZigCrdt ZigCrdt;

ZigCrdt *crdt_create(void);
void crdt_destroy(ZigCrdt *crdt);

int32_t crdt_apply_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

int32_t crdt_merge_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

int32_t crdt_materialize_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

void crdt_free(ZigCrdtBuffer buffer);
```

Input envelopes must include trusted host context:

```json
{
  "context": {
    "appId": "notebook-app",
    "notebookId": "notebook_...",
    "actorId": "actor_...",
    "actorKind": "human | ai",
    "permissions": ["notebook.write"],
    "baseFrontier": []
  },
  "operation": {}
}
```

Generated apps do not construct this full trusted envelope. The host builds it after bridge validation.

### G9. Reference host first

The reference host must implement the notebook bridge contract before native hosts are considered complete.

Required reference-host capabilities:

- open/create notebook;
- apply local edit;
- apply AI proposal;
- accept/reject proposal;
- export snapshot;
- import/sync update;
- query current materialized notebook JSON;
- audit accepted/rejected operations;
- run Loro fixture parity tests.

Native hosts and server must match reference-host bridge responses for shared fixtures.

## 4. Non-goals

- No arbitrary CRDT choice per generated app.
- No direct app-level WebSocket or fetch sync.
- No generated-app npm, Rust, WebAssembly, React, TypeScript, Vite, or bundler dependency.
- No raw SQL access for generated apps.
- No rich notebook editor UI requirement in the first CRDT package milestone.
- No direct P2P requirement before the server/reference-host sync path is stable.
- No AI canonical writes without permission and audit.

## 5. Security requirements

- Host derives `app_id`; callers cannot choose it.
- Host derives or authenticates actor identity.
- Every op is checked against app install permissions and notebook ACL.
- AI actors default to proposal-only permissions.
- Operations outside declared schema are rejected before merge.
- Merged materialized state is validated after merge.
- Oversized operations, documents, and update batches are rejected by resource budgets.
- Sync endpoints require authenticated transport and replay protection.
- CRDT update import must be idempotent and duplicate-safe.
- All accepted and rejected operations are audited.

## 6. Testing requirements

Standalone package:

- Zig unit tests for parser, operation validation, merge, materialization, replay, and C ABI memory ownership.
- Differential tests against Loro-generated fixtures.
- Randomized convergence tests for supported operations.
- Duplicate/out-of-order update tests.
- Schema migration tests.
- Malicious/oversized input tests.

Reference host:

- Bridge contract tests for every `notebook.*` method.
- SQLite persistence tests.
- Permission-denied and AI proposal approval tests.
- Snapshot/import/export tests.
- Loro fixture parity tests.

Native/server:

- Each host must pass shared notebook bridge fixtures.
- Each host must verify DB rows for accepted/rejected CRDT operations.
- Realtime transport tests may be platform-specific, but materialized state must match the reference host.

## 7. Implementation phases

### Phase 0. Research and fixture lock

- Keep `external-lib/loro` as a pinned reference checkout.
- Record license and dependency review.
- Build a small fixture generator that produces JSON fixtures from Loro.
- Decide the exact supported notebook profile.

### Phase 1. Standalone Zig package

- Create `zig-crdt/`.
- Implement operation schema, actor context, notebook materialization, and deterministic replay.
- Pass fixture parity for maps, text, ordered cells, metadata, comments, and proposals.

### Phase 2. Reference host bridge

- Add `notebook.*` methods to reference host.
- Add DB migrations and schemas.
- Add control tools for notebook snapshot/query/replay.
- Add bridge fixtures.

### Phase 3. Server and native host integration

- Link/load `zig-crdt` next to `zig-core`.
- Add native bridge dispatch.
- Add persistence and audit.
- Add smoke tests per platform.

### Phase 4. Realtime collaboration

- Add server sync sessions.
- Add ephemeral presence/cursor channel.
- Add reconnect and offline catch-up.
- Add stress tests with multiple human and AI actors.

## 8. Acceptance criteria

The feature is not complete until:

- `zig-crdt` passes standalone Zig tests.
- Loro parity fixtures pass in CI.
- Reference host implements and contract-tests all accepted `notebook.*` methods.
- SQLite and Postgres schemas are both updated.
- At least one native host and the server load the same Zig CRDT package.
- Human-human concurrent edits converge.
- Human-AI proposal/approval flow is audited and permission checked.
- Offline edits sync after reconnect without losing accepted state.
- Generated apps can use notebook collaboration through `AppRuntime.call` only.
