# Phase 2 — `system.describe` (learn the surface through the surface)

**Theme:** expose the Phase-1 catalog as a normal command so every front-end
discovers the command set the *same way* — no per-platform discovery code.

**Risk:** very low. Pure read of static catalog state.
**Replay impact:** none (deterministic read; emits no events, mutates nothing).

## Why a command (and not a side-channel)

The whole thesis is "one door." Discovery must go through the same
`handle(CoreCommand)` path as execution, so:

- the CLI (FFI bin), the console (`/bridge`), and an agent all call it
  identically;
- it inherits the same auth/actor model — the catalog can be **filtered by the
  caller's role** (an Editor sees only what an Editor may run);
- it shows up in the catalog itself (self-referential, discoverable).

## Command shape

Register in the table like any other (Phase 1 makes this one descriptor +
handler):

```jsonc
// request
{ "name": "system.describe",
  "payload": {
    "names":     ["query.execute"],   // optional filter
    "namespace": "applet",            // optional filter
    "tier":      "public",            // optional max visibility filter
    "include_inner": false,           // include ctx.* reference entries?
    "for_role":  null                 // optional: describe as-if this role (default = caller)
  } }

// response
{ "ok": true,
  "payload": {
    "catalogVersion": "sha256:...",   // stable hash of the emitted catalog
    "runtimeVersion": "…",
    "commands": [ /* CommandDescriptor[] sorted by name */ ],
    "roles":    [ /* the role enum, for clients */ ],
    "tiers":    ["public","operator","admin","debug"]
  } }
```

### Default filtering (safety)

- Without auth elevation, `system.describe` returns only commands the **caller's
  role** can run *and* whose `visibility` the caller's surface is allowed to see
  (see [10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md)).
- `debug`/`control.*` entries are omitted unless the corresponding feature is
  compiled in **and** the caller is privileged.
- `for_role` lets an operator preview another role's surface (read-only; does not
  grant anything).

## Steps

### P2.1 — Add the handler

`cmd_system_describe(core, cmd)` reads the static catalog, applies the payload
filters and the caller-role/visibility filter, computes `catalogVersion` (hash of
the sorted, serialized descriptors), and returns the response. No storage writes.

### P2.2 — Register + authorize

Add `("system.describe", …)` to the table with `visibility: public` and a broad
read role set (every role may *describe* what it can run). Add the matching
`authorize()` entry (or rely on the unified table from P1.4).

### P2.3 — Determinism

`catalogVersion` must be a function only of the compiled catalog (sorted), so the
same build always yields the same hash. No clock, no random. This makes it
replay-safe and lets the public-contract gate (Phase wiring in
[11](11-SCHEMAS-AND-CONTRACT.md)) assert the hash.

### P2.4 — Tests

- `system.describe` with no filter returns every command the caller may run.
- Filters (`names`, `namespace`, `tier`) narrow correctly.
- A Viewer does **not** see `quota.set` in the result.
- `catalogVersion` is stable across repeated calls and process restarts.

## Deliverables

- `cmd_system_describe` + registry/authorize entries.
- Request/response schemas under `schemas/commands/`.
- Tests for filtering, role-scoping, and hash stability.

## Validation

```sh
cd forge
cargo test -p forge-core
# manual smoke once Phase 3 lands, or via core-invoke:
echo '{"request_id":"r1","actor":{"actor":"cli","role":"owner"},
  "workspace_id":"ws-demo","name":"system.describe","payload":{}}' \
  | cargo run -p forge-ffi --bin core-invoke
```

## Sibling: `system.trace`

`system.describe` answers *"what commands exist?"*. Its sibling `system.trace`
answers *"what effects did a run actually perform?"* by surfacing the existing
`RecordedCall`/`RunRecord` journal (`forge/crates/domain/src/run.rs:49`) as a
read-only outer command. Same pure-read, replay-safe pattern; build it alongside
`system.describe`. Full design — and why it is the bridge between the outer and
inner doors — is in
[14-EFFECT-SURFACE-AND-OBSERVABILITY.md](14-EFFECT-SURFACE-AND-OBSERVABILITY.md).

## Exit criteria

- Any front-end can obtain the full, role-scoped catalog via one command.
- The catalog hash is deterministic and stable.
- No privileged command leaks to an under-privileged caller's describe result.
