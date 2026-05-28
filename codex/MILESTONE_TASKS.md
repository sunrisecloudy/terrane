# Codex Implementation Plan

> **This file is a pointer.** The single source of truth for the Codex implementation plan is **`docs/09_CODEX_IMPLEMENTATION_PLAN.md`**. It contains the full milestone list (Milestone 0–14, MCP-0 through MCP-6, DB-0 through DB-5).
>
> This redirect exists because earlier revisions kept a separate per-Codex copy that drifted from `docs/09`. Don't add new content here; add it to `docs/09` instead.

## Quick map (refer to `docs/09` for details)

| Range | Theme | Doc |
|---|---|---|
| Milestone 0–10 | v0.1 build: skeleton → Zig core → runtime → server → desktop → mobile → hardening | docs/09 §2–§12 |
| MCP-0–6 | v0.2 Codex control plugin and dev control plane | docs/09 §"Codex control plugin milestones" |
| Milestone 11–14 | v0.3 trust, rollback, capabilities, snapshot/replay, repair loop | docs/09 §"v0.3 implementation milestones" |
| DB-0–5 | v0.4 database, persistence, migrations, backup, Codex DB tools, native adoption | docs/09 §15 |

## Codex working agreements

See `AGENTS.md` at the repo root. It supersedes the older "Codex rules" subsection that used to live in this file.

## Why this redirect

Two milestone lists were drifting (this file vs `docs/09`). The repo-level instructions in `AGENTS.md` point to `docs/09` as authoritative. This file is kept only to avoid breaking links from older Codex sessions.
