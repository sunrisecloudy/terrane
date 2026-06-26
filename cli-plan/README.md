# cli-plan — The Forge Unified CLI

This folder is the plan for **one self-describing command surface** that every
Terrane platform — the five native shells, the web runtime, the local HTTP
server, and external agents — drives through the *same door*, and a generic CLI
+ web console + agent adapter built on top of it.

## The thesis in one sentence

Terrane already funnels **every action** through a single command facade
(`WorkspaceCore::handle(CoreCommand) -> CoreResponse`) exposed over a single C
ABI (`forge_core_handle_command`) and a single data-driven registry
(`COMMANDS`). The plumbing is done; what is missing is **self-description** — the
metadata that lets a human, a UI, or an LLM agent *learn every command*. Add
that one keystone and a complete CLI, an auto-generated web console, and an
agent tool surface all fall out of it.

## Read in this order

| File | What it covers |
| --- | --- |
| [00-OVERVIEW.md](00-OVERVIEW.md) | Vision, goals, non-goals, the two-surface model, why this is low-risk. |
| [01-FINDINGS.md](01-FINDINGS.md) | The evidence: what already exists, with `file:line` citations. |
| [02-PLATFORM-OVERVIEW.md](02-PLATFORM-OVERVIEW.md) | **The complete head-on platform overview** — every surface and how they converge on one facade. |
| [03-TARGET-ARCHITECTURE.md](03-TARGET-ARCHITECTURE.md) | The unified-CLI target: catalog → introspection → CLI → console → agent. |
| [04-COMMAND-CATALOG.md](04-COMMAND-CATALOG.md) | The keystone: the command descriptor model + a full enumeration of today's commands. |
| [05-PHASE-1-SELF-DESCRIBING-REGISTRY.md](05-PHASE-1-SELF-DESCRIBING-REGISTRY.md) | Make the registry carry metadata. |
| [06-PHASE-2-INTROSPECTION-COMMAND.md](06-PHASE-2-INTROSPECTION-COMMAND.md) | `system.describe` — learn the catalog through the same surface. |
| [07-PHASE-3-CLI-FRONTEND.md](07-PHASE-3-CLI-FRONTEND.md) | The generic `forge` CLI: `commands`, `describe`, `run`. |
| [08-PHASE-4-WEB-CONSOLE.md](08-PHASE-4-WEB-CONSOLE.md) | The auto-generated web command console over `/bridge`. |
| [09-PHASE-5-AGENT-ADAPTER.md](09-PHASE-5-AGENT-ADAPTER.md) | Project the catalog into LLM tool definitions. |
| [10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md) | Visibility tiers, role gating, effectful calls, determinism. |
| [11-SCHEMAS-AND-CONTRACT.md](11-SCHEMAS-AND-CONTRACT.md) | Schema source of truth, drift gates, public-contract integration. |
| [12-MILESTONES.md](12-MILESTONES.md) | Sequencing, effort, exit criteria, validation commands. |
| [13-OPEN-QUESTIONS.md](13-OPEN-QUESTIONS.md) | Decisions to lock before/while building. |
| [14-EFFECT-SURFACE-AND-OBSERVABILITY.md](14-EFFECT-SURFACE-AND-OBSERVABILITY.md) | The two-door decision: should JS host-calls pass through the same door? `system.trace`, agent-via-UI. |

## Status

Planning only. No implementation in this branch yet.

**Code audit:** 2026-06-26 — `file:line` citations and the command/role tables
in `01-FINDINGS.md` / `04-COMMAND-CATALOG.md` were verified against the workspace
in this worktree. Key corrections: **42** `COMMANDS` entries (not ~46);
`WorkspaceCore::handle` lives in `workspace.rs:986`; public-contract
`CORE_COMMANDS` drifts from the live registry (F11). Re-verify before coding if
`forge-core` moves.

## Naming

The initiative is **Forge Unified CLI**. The shipped binary stays `forge`
(today `forge demo`); this plan grows it into `forge <command>`.
