# Phase 5 — The agent adapter

**Theme:** project the catalog into LLM tool/function definitions so an agent can
drive Terrane with no bespoke glue. This is the "build/buy another agent" piece —
and it is *nearly free* once Phases 1–2 exist, because an agent tool definition is
essentially `{ name, description, inputSchema }`, which is exactly a
`CommandDescriptor`.

**Risk:** low. A pure projection + a thin executor that reuses `forge run` /
`/bridge`.
**Replay impact:** none.

## The mapping

```
CommandDescriptor                         LLM tool definition
─────────────────                         ───────────────────
name             ───────────────────────▶ name           (e.g. "query.execute"
                                                           → "query_execute")
summary          ───────────────────────▶ description
payload_schema   ───────────────────────▶ input_schema / parameters
visibility/roles ───────────────────────▶ which tools are offered to this agent
mutates/effectful ──────────────────────▶ description hints / confirm policy
```

So the adapter is: `system.describe` → filter by an **allowed tier + role for the
agent** → emit one tool per command → on a tool call, execute via the same
transport the CLI uses.

## Why this matters

- An agent **learns every command** from the catalog — no curated, hand-written
  tool list that rots. New command → new tool, automatically.
- The agent is **safely scoped** by tier/role: give a support agent `public`-only
  tools; give an admin agent more. The catalog enforces the boundary.
- Works for **any** agent runtime that accepts JSON-schema tools (the Claude API
  tool-use shape maps directly: `name` / `description` / `input_schema`).

## Steps

### P5.1 — The projector

A function (Rust in `forge-cli`, or JS in `tools/`) `catalog_to_tools(catalog,
{ tier, role })` returning tool definitions. Sanitize names for tool-name charset
(`.` → `_`) and keep a reverse map. Attach `mutates`/`effectful` to the
description so the model knows what is destructive.

### P5.2 — The executor

On a tool call `query_execute({collection:"notes"})`, the executor:

1. maps the tool name back to `query.execute`,
2. validates args against `payload_schema` (reject early, like `--dry-run`),
3. issues the command via local core or `/bridge` (reuse Phase 3 transport),
4. returns the `CoreResponse` (or `CoreError`) as the tool result.

### P5.3 — Guardrails

- **Tier ceiling per agent**: the projector never emits tools above the agent's
  configured tier; the executor re-checks server-side (defense in depth).
- **Confirm policy** for `mutates`/`effectful` tools (optional human-in-the-loop).
- **Audit**: mutating tool calls already land in the SC-12 audit log via the
  normal command path (`audit.query` can review them) — no special wiring.

### P5.4 — Reference integration

Ship a small example: an agent loop that calls `system.describe`, registers the
`public` tools, and answers "list my notes" by calling `query_execute`. Use it as
the conformance smoke for the adapter.

## Relationship to the rest of the platform

- The adapter is **just another front-end** in the
  [02-PLATFORM-OVERVIEW.md](02-PLATFORM-OVERVIEW.md) picture — same facade, same
  catalog, same transports.
- It is **public-engine only**: no SaaS identity. If a hosted product wants an
  agent, it consumes this adapter through `artifacts/public-contract.json` or a
  pinned checkout, per `CLAUDE.md`.

## Deliverables

- `catalog_to_tools(...)` projector + name sanitization/reverse map.
- An executor reusing the Phase 3 transport with schema pre-validation.
- Tier/role scoping + optional confirm policy.
- A reference agent example + smoke test.

## Validation

```sh
# project the public tools and run the example agent loop against a seeded core
cargo run -p forge-cli -- commands --tier public --json   # the raw catalog the
                                                          # projector consumes
```

## Exit criteria

- The tool list offered to an agent equals the catalog filtered by the agent's
  tier/role — generated, not hand-maintained.
- A tool call executes the corresponding command and returns its response.
- No tool above the agent's tier is ever emitted or executed.
