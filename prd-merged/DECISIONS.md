# Merge Decision Record

**Date:** 2026-06-12 · **Decided by:** Founder (stakeholder Q&A) unless marked otherwise
**Sources merged:** `local_first_util_2/` (F, "Forge" pack) · `local_first_utility_prd_pack/` (P, "Utility" pack)

## Where the packs already agreed (adopted without discussion)

Single Rust core owning all product-critical behavior; thin native shells with no business logic; UniFFI + wasm-bindgen generated bindings; TypeScript as the user/LLM language with a strict profile; no npm at v1 (curated stdlib + local modules); capability-based sandbox with host-only `ctx` APIs; Loro CRDT; SQLite as single-file workspace format and KV/oplog substrate; dynamic logical schemas as data with **stable field IDs** and additive evolution; unknown-field preservation as a normative rule; tombstones with hard-purge classes; OS keychain for secrets, never synced/logged/in-context; open core + optional commercial central services + self-hosting; no mandatory E2E encryption v1 (project-level keys supported); LLM pipeline that cannot bypass verification or grant its own permissions.

## Stakeholder decisions (resolve the conflicts)

| # | Question | F said | P said | **Decision** |
|---|---|---|---|---|
| D1 | First deliverable / reference | macOS desktop first (M1) | Web first (ADR-003) | **Neither shell first: headless e2e template first** — CLI harness speaking the real shell contract proves the full vertical slice; every platform then implements against conformance kits. |
| D2 | Generated-app UI model | Declarative ~26-component tree, native renderers | Minimal output UI v1, native UI bridge later | **Declarative component tree + "renderer zero"** (throwaway DOM renderer ~week 3) to keep the headless contract honest against real UI. Script outputs render through the same protocol. |
| D3 | Sync topology | Home server (managed cloud or embedded desktop), WebSocket | P2P-first WebRTC + optional relay (ADR-010) | **Home server (cloud or embedded).** Server-enforced RBAC, CI-testable in one process. Frames are transport-agnostic; direct device-to-device is a v2 candidate, not v1. |
| D4 | JS engine | QuickJS everywhere v1, JSC at iOS time (CR-2) | QuickJS-Wasm everywhere (ADR-006) | **JSC on Apple from day one; QuickJS elsewhere** (rquickjs native, QuickJS-WASM web). Dual-engine conformance suite from week one; divergence is release-blocking. |
| D5 | v1 platform scope | macOS + Windows + web | All 8 platforms phased | **macOS + web (+ Linux headless server); Windows fast-follow v1.x; then iOS → Android.** Tauri fallback gate for Windows retained. |
| D6 | Marketplace | Deferred (templates v1.x, no marketplace) | Centralized marketplace as committed phase | **Marketplace committed**: beta at M4, source-visible editable installs, publisher auth, signing-ready format, signing enforcement deferred. Not inside the iOS app at launch. |
| D7 | Type-check placement | tsgo sidecar desktop/server; cloud/home-server check for web/mobile | Offline compiler required everywhere | **Fully offline everywhere**: SWC in-core; tsgo sidecar on desktop/server; `tsc` in a lazy-loaded worker on web; bundled checker on mobile. No cloud dependency anywhere in the pipeline. |
| D8 | Accounts | Account-centric cloud (implicit) | Login optional | **No login for local use; account (passkeys/OAuth) only for cloud services** (managed sync, relay, bundled credits, publishing). |

## Editorial resolutions made during the merge (review these)

| # | Topic | Resolution | Rationale |
|---|---|---|---|
| E1 | Container model: F "workspace/collection/applet" vs P "workspace→project as portable SQLite file" | **Workspace** is the unit of sync/membership/export and is itself the single portable SQLite file; applets/scripts live inside it. No separate "project" layer. | One container concept; P's portability requirement preserved via DL-24 export. |
| E2 | Runnable shapes: F event-driven applets vs P `main(ctx, input)` scripts | Both ship: **applet** (UI, event-driven) and **script/automation** (run-to-completion, deterministic mode + replay) — CR-7/CR-8. | The deterministic-run/replay machinery (P) is too valuable to drop; UI applets (F) are the product's face. |
| E3 | Projection: F `records` JSONB table + expression indexes vs P `index_rows` materialized index table | F's projection + expression/FTS5 indexes as primary design; P's index **lifecycle** (resumable, rebuildable, planner warnings) adopted on top. | Leverages SQLite instead of re-implementing indexing; keeps DL-6 rebuild escape hatch. |
| E4 | Query surface: F typed DSL vs P SQL-like strings | DSL is the applet-facing API; the same validated subset is exposed as SQL-like strings for the data browser/SDK (`query.execute`). | One planner, two skins. |
| E5 | RBAC roles | P's customizable RBAC with defaults (Owner/Maintainer/Editor/Runner/Viewer/Auditor/Reviewer) replaces F's fixed owner/editor/viewer. | Strictly more general; marketplace needs Reviewer anyway. |
| E6 | Local LLM | Both P's LM Studio adapter (M0/M1, zero-install) and F's in-core engine + model manager (M4). | Adapter is nearly free and unblocks offline dev immediately. |
| E7 | Crate/command naming | P-04's crate layout and command/event/error model adopted wholesale into CR §2–3. | It was the more concrete spec; F had no equivalent. |
| E8 | Codename | "Forge" kept as working codename; brand TBD (Terrane assets exist). | Open question #1 in master PRD. |

## Amendments from Review 001 (2026-06-12, `review/001-prd-merged-review.md`)

| # | Finding | Resolution |
|---|---|---|
| R1 (P1) | Merged PRD contradicted normative v0.4 repo rules (`AGENTS.md`/`docs/00_PRD.md` forbid TS for generated apps) | Supersession doc `docs/00_V1_PIVOT.md` created; banner added to `docs/00_PRD.md`. v0.4 = legacy/prototype reference; `prd-merged/` = normative. |
| R2 (P1) | M0 too large for one milestone | Split: **M0a** executable spine (4–5 wks) / **M0b** conformance & template (4–5 wks) — PRD 09 §1, master §11. |
| R3 (P1) | TS→SWC→QuickJS-WASM→Rust-ctx must be the central acceptance test; JSC co-equal-from-day-one dilutes it | **D4 amended:** the QuickJS-WASM spine is the M0a proof and the product's central acceptance test (CR-12); JSC still lands inside M0 (M0b, with the conformance suite) — not deferred to iOS time. |
| R4 (P2) | `JSONB` is not SQLite-native | Projection column changed to `TEXT` JSON + JSON1; Postgres `JSONB` noted as server-scale option only — DL §4. |
| R5 (P2) | Offline web type-check conflicts with 6 MB core budget | `tsc` worker is a lazy optional artifact **outside** the core budget (≤ 10 MB gzipped, cached; honest "pending download" fallback, never silent skip) — CR-15, CR §8, PRD 09 §4. |
| R6 (P2) | Server-readable vs encryption needed a sharp product rule | Explicit owner-chosen **server-visibility modes**: server-readable workspace (default; search/share/server-LLM/scheduling) vs encrypted workspace (ciphertext to server; features disabled/degraded) — SS-14, DL-25, master non-goals. |

## Explicitly dropped

- P2P/WebRTC transport at v1 (D3) — protocol designed not to preclude it.
- F's "web/mobile type-check routes to home server/cloud" (D7).
- P's 8-platform v1 commitment (D5) — platforms remain on the roadmap, not in v1 scope.
- F's "no marketplace" non-goal (D6).
- Electron/Flutter/React Native as primary shells (both packs already rejected; Tauri remains only as the Windows fallback gate).
