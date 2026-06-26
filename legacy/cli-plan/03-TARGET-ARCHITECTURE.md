# 03 — Target Architecture

## One catalog, four projections

```
                    ┌───────────────────────────────────────────┐
                    │  COMMAND CATALOG  (Phase 1 — the keystone) │
                    │  per command: name, summary, payload &     │
                    │  response schema, role(s), capability,     │
                    │  mutates?, effectful?, visibility tier     │
                    └───────────────┬───────────────────────────┘
                                    │ exposed through the SAME facade
                                    ▼
                    ┌───────────────────────────────────────────┐
                    │  system.describe  (Phase 2)                │
                    │  a normal CoreCommand that returns the     │
                    │  catalog as JSON                           │
                    └──┬─────────────┬──────────────┬───────────┘
                       │             │              │
          ┌────────────▼──┐ ┌────────▼───────┐ ┌────▼─────────────┐
          │ forge CLI     │ │ web console    │ │ agent adapter    │
          │ (Phase 3)     │ │ (Phase 4)      │ │ (Phase 5)        │
          │ commands/     │ │ form-per-      │ │ catalog → LLM    │
          │ describe/run  │ │ command UI     │ │ tool defs        │
          └───────┬───────┘ └───────┬────────┘ └────────┬─────────┘
                  │                 │                   │
                  └─────────────────┼───────────────────┘
                                    ▼
                      WorkspaceCore::handle(CoreCommand)
                      (FFI core-invoke  OR  server /bridge)
```

The catalog is authored once. The CLI, console, and agent are **projections** of
it — they contain no command knowledge of their own, so they cannot drift.

## Component 1 — The catalog (Phase 1)

A descriptor table that lives next to `COMMANDS` and is the single source of
truth for command metadata. Full model and field-by-field rules in
[04-COMMAND-CATALOG.md](04-COMMAND-CATALOG.md). Key design choices:

- **Co-located with dispatch.** The descriptor sits beside `("name", handler)` so
  adding a command and describing it are one edit, and a missing descriptor is a
  compile/test failure — not silently undocumented.
- **References, not copies.** `required_roles` derives from the *same* table
  `authorize()` uses (F3). Schemas *reference* `forge/schema` / `schemas/*.json`
  rather than re-declaring shapes (F9). One fact, one home.
- **Pure data.** The catalog is static; building/serving it has no side effects,
  so `system.describe` is replay-safe.

## Component 2 — `system.describe` (Phase 2)

A new, read-only command registered in the table like any other. It returns:

- the full catalog (optionally filtered by `name`, `tier`, or `namespace`), and
- a stable `catalogVersion` / hash so clients can cache and detect change.

Because it travels through `handle()`, **every** front-end learns the surface the
same way: the CLI calls it over the FFI bin, the console calls it over `/bridge`,
an agent calls it over either. No platform-specific discovery code.

## Component 3 — The `forge` CLI (Phase 3)

Grows `forge-cli` from `demo`-only into a generic front-end:

```
forge commands [--tier ...] [--json]        # list (from system.describe)
forge describe <name> [--json]              # one command's schema + roles
forge run <name> [--payload <json>|-]       # issue a command, print response
forge demo                                  # unchanged spine gate
```

`run` builds the envelope, opens a core (FFI) or targets a server URL, calls
`handle`, and prints `CoreResponse`. Details + UX in
[07-PHASE-3-CLI-FRONTEND.md](07-PHASE-3-CLI-FRONTEND.md).

## Component 4 — The web console (Phase 4)

A static page served by (or alongside) `forge-server`:

1. `GET` the catalog via `POST /bridge` `{ name: "system.describe" }`.
2. Render a left-nav of commands grouped by namespace, filtered by visibility
   tier.
3. For a selected command, render a **form generated from its payload schema**.
4. Submit → `POST /bridge` → render the `CoreResponse` (and drain events).

No command logic in the page — it is a thin renderer of the catalog. Details in
[08-PHASE-4-WEB-CONSOLE.md](08-PHASE-4-WEB-CONSOLE.md).

## Component 5 — The agent adapter (Phase 5)

A small projector (Rust or JS) that turns the catalog into LLM tool/function
definitions — one tool per command, `inputSchema` = the command's payload
schema, description = its summary, gated by tier. An agent then drives Terrane
with no bespoke integration. Details in
[09-PHASE-5-AGENT-ADAPTER.md](09-PHASE-5-AGENT-ADAPTER.md).

## Transports (unchanged)

The CLI and agent can reach a core two ways, both already present:

| Transport | Use | Source |
| --- | --- | --- |
| **FFI `core-invoke` bin** | local, in-process, no server | `forge/crates/ffi/src/bin/core_invoke.rs` |
| **HTTP `POST /bridge`** | a running server / remote-ish | `forge/crates/server/src/lib.rs:92` |

The CLI defaults to opening a local core directly (it links `forge-core`); a
`--server <url>` flag targets `/bridge` instead. The console always uses
`/bridge`.

## What changes vs. what stays

| Stays exactly as-is | Changes / is added |
| --- | --- |
| `cmd_*` handler bodies | a descriptor next to each registry entry |
| `authorize()` semantics | (ideally) `authorize` reads the shared role table |
| dispatch ordering (RBAC→dispatch) | `system.describe` added to the table |
| replay determinism | a new pure introspection read |
| native shells' FFI calls | the `forge` binary gains subcommands |
| `/bridge` protocol | a static console page that uses it |
| public-contract *names* | the export now also emits catalog metadata + a drift gate |

## Design invariants

1. **Single source of truth.** Every consumer reads the catalog; none hard-codes
   command knowledge.
2. **Same door for discovery and execution.** `system.describe` is itself a
   command — no side-channel.
3. **Tiered exposure.** Visibility metadata gates what each front-end shows; the
   public web surface never sees `admin`/`debug` by default.
4. **No behavior drift.** Front-ends issue only pre-existing commands; the catalog
   is wired into the verify gate so metadata cannot rot.
5. **Public-engine only.** Nothing here imports or assumes the private SaaS plane.
