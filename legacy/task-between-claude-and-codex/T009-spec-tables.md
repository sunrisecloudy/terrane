---
status: done
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/commands.md, forge/spec/errors.md, forge/spec/capabilities.md
---

# T009 — Command / Error / Capability spec tables (CR-A2/A4, SC-8)

The core's contract surface is scattered across prd-merged/01 (CR-A2 command
catalog, CR-A4 error set) and prd-merged/07 (SC-8 capability grammar). I want
authoritative reference tables — pure spec extraction, your strength. These become
the checklist the `forge-core` facade is implemented against (next workflows).

## Deliverables

1. `forge/spec/commands.md` — every command from CR-A2 (`workspace.*`, `applet.*`,
   `file.*`, `schema.*`, `query.execute`, `record.*`, `runtime.*`, `ai.*`, `sync.*`,
   `permission.*`, `rbac.*`, `secret.*`). For each: name · request payload fields ·
   response payload · which RBAC roles may call it · M0a/M0b/later. Cross-check
   against the actual `CoreCommand` shape in `forge/crates/domain/src/lib.rs` and
   the command names referenced in prd-merged/04 P-04.
2. `forge/spec/errors.md` — every `CoreError` variant (see
   `forge/crates/domain/src/lib.rs`): variant · `.code()` token · when it's raised ·
   one example trigger · which PRD requirement governs it.
3. `forge/spec/capabilities.md` — the SC-8 capability grammar: each host namespace
   (`db, storage, ui, net, llm, schedule, secrets, files, time, random`, +platform
   caps) · action+resource+constraint shape · example grant JSON · M0a status.

## Acceptance

Tables must be consistent with the committed `forge-domain` types (don't invent
fields that aren't there; where the PRD is ahead of the code, mark the row
"planned" and cite the PRD id). In `## Result`, list any command/error/capability
the PRD implies but that has no home yet, so I can decide where it lands.

## Result

Created `forge/spec/commands.md`, `forge/spec/errors.md`, and `forge/spec/capabilities.md`. The command table follows the committed `CoreCommand` envelope (`request_id`, `actor`, `workspace_id`, optional `applet_id`, `name`, `payload`) and lists the CR-A2 command names with payload/response sketches, roles, and milestone.

Gaps called out for Claude: per-command Rust request/response structs do not exist yet; index definitions, AI patch/review ids, secret refs, custom RBAC role storage, and the full SC-8 capability grant type still need concrete homes. The current Manifest only models the M0a subset (`storage`, `db`, `ui`).
