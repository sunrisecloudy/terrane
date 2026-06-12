# Claude ⇄ Codex Task Board

Handoff protocol between Claude (implementing `forge/` per `prd-merged/`) and Codex (reviewer + collaborator).

## Protocol

- One task per file: `T###-<slug>.md`, numbered in order of creation.
- Frontmatter fields: `status: requested | in-progress | done | blocked`, `requester`, `assignee`, `deliverable` (paths).
- **Codex:** claim a task by setting `status: in-progress`, deliver by writing the files listed under *Deliverable* (and/or a `## Result` section in the task file), then set `status: done`. If blocked or the task is unclear, set `status: blocked` with a `## Question` section.
- **Claude:** checks this folder between workflow stages (every few minutes while building), integrates `done` deliverables, answers `blocked` questions, and files new tasks.
- Codex can also file tasks *for Claude* here (set `assignee: claude`).
- Commit reviews keep going to `review/` as `NNN-<slug>.md` — that flow is unchanged.

## Context for Codex

- Normative spec: `prd-merged/` (the repo's `AGENTS.md`/`docs/00_PRD.md` v0.4 rules are **superseded** — see `docs/00_V1_PIVOT.md`).
- Active milestone: **M0a executable spine** (`prd-merged/09-roadmap-quality-gates-prd.md` §1):
  `TS → SWC → QuickJS → Rust capability ctx → SQLite write → UI tree patch → deterministic replay, all offline.`
- Workspace: `forge/` (Rust 1.96, rusqlite 0.40.1 bundled, loro 1.13.1, rquickjs 0.12.0). Branch: `forge-m0a`.
- Build/test: `cd forge && cargo test`. Never commit `forge/target/` (gitignored). Never `git add -A` (the repo has unrelated dirty files on purpose).

## Task index

| Task | Status | Title |
|---|---|---|
| T001 | requested | Hostile TypeScript corpus for sandbox tests |
| T002 | requested | `@forge/std` ctx TypeScript type definitions |
| T003 | requested | SWC crate selection research for in-core TS strip |
