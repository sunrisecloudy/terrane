# 00 — Overview

## Problem statement

Terrane runs on five native shells (macOS, iOS, Android, Windows, Linux), a web
runtime, and a local HTTP server. We want:

1. **One call system** that any platform can drive — so a platform learns the
   command surface *once* and gets every action for free.
2. **A CLI that can run every action**, and is **self-describing** so an agent
   (or a person, or a tool) can *learn every command* without reading source.
3. **A web "scrape" / operator interface** built on top of those same commands,
   so the UI never drifts from what the core actually does.

The good news, established in [01-FINDINGS.md](01-FINDINGS.md): goal (1) is
**already true**. Every shell already invokes the core through one C ABI, and
the core already routes every command through one data-driven registry. The work
is not plumbing — it is **making that surface describe itself**, then projecting
that self-description into a CLI, a console, and an agent adapter.

## The two surfaces (don't conflate them)

The word "command" and the word "call" name **two related but distinct
surfaces**. The plan covers both, but keeps them separate because they have
different audiences and different trust levels.

| Surface | Who issues it | Entry point | Audience |
| --- | --- | --- | --- |
| **Outer — CoreCommands** | shells, CLI, server clients, agents | `WorkspaceCore::handle(CoreCommand)` | operators / tools / agents |
| **Inner — host-calls (`ctx.*`)** | a *running applet* asking the host to do something | the bridge / runtime host-call ABI | generated apps |

The unified CLI is primarily about the **outer** surface (operator/agent-driven
actions). The **inner** `ctx.*` surface (`ctx.db`, `ctx.net`, `ctx.files`,
`ctx.ui`) is what an app uses at runtime; it belongs in the catalog for
*documentation and capability reasoning*, but it is never something an operator
"runs" directly. Keeping the two clearly tiered is what prevents a generated app
from reaching an admin command, and what lets the console safely expose only the
operator surface.

## Goals

- A single **machine-readable command catalog** that is the source of truth for:
  name, summary, payload schema, response schema, required role(s), capability
  scope, mutates-state?, effectful?, and a visibility tier (public / operator /
  admin / debug).
- An **introspection command** (`system.describe`) that returns the catalog
  through the *same* `handle()` surface every platform already uses — no new
  per-platform discovery path.
- A **generic `forge` CLI**: `forge commands`, `forge describe <name>`,
  `forge run <name> --payload <json>` — a thin wrapper over the facade.
- An **auto-generated web console** that renders a form per command from its
  schema and submits through the existing server `/bridge` endpoint.
- An **agent adapter** that projects the catalog into LLM tool definitions, one
  tool per command, so an agent can drive Terrane with no bespoke glue.
- **Zero drift**: the catalog is wired into the existing public-contract verify
  gate so a command cannot ship without metadata.

## Non-goals

- **Not** a rewrite of the dispatch or RBAC model. The catalog *references*
  existing `authorize()` rules and existing schemas; it does not re-author them.
- **Not** changing app-visible runtime behavior or replay determinism. The CLI
  is an alternative *front-end* to commands that already exist.
- **Not** a new network protocol. The server already exposes a generic `/bridge`
  POST; the console reuses it.
- **Not** SaaS / control-plane concerns. This is public-local-engine surface
  only (see `CLAUDE.md` on the public/premium split). No identity, billing, or
  hosted coordination enters here.
- **Not** exposing privileged/admin commands to the public web surface by
  default — visibility tiers gate that (see [10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md)).

## Why this is low-risk

- The **command handlers do not change** — we add a metadata column next to each
  registry entry and a read-only introspection command.
- The CLI and console are **new front-ends** over an unchanged facade, so they
  cannot regress existing shells.
- The introspection command is **pure** (reads static catalog state), so it does
  not threaten deterministic replay.
- Each phase ships independently and is individually useful (the CLI is valuable
  even before the console exists).

## The shape of the win

> ~80% of the value is **Phase 1** (make the registry self-describing). The CLI,
> the web console, and the agent adapter are mostly *projections* of that one
> catalog. Build the keystone once; the rest is wiring.

See [03-TARGET-ARCHITECTURE.md](03-TARGET-ARCHITECTURE.md) for the design and
[12-MILESTONES.md](12-MILESTONES.md) for sequencing and effort.
