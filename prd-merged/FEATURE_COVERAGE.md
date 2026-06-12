# Feature Coverage Map — docs/ → forge v1

**Purpose:** the durable backlog for the goal *"implement every feature in the
original `docs/` PRD."* Built from a fan-out extraction of all 37 `docs/` specs
(832 atomic requirements) cross-referenced against `prd-merged/` (the v1 plan)
and the built `forge/` crates. Organized by **feature area** (more actionable
than atomic rows), each with status, source docs, the governing `prd-merged`
requirement, and the `forge/` location.

**Status legend:** `done` (built + tested in forge/) · `partial` (substrate
exists, gaps remain) · `planned` (in prd-merged, not built) · `dropped`
(legacy Zig/WebView-only; intentionally not carried — recorded, not built).

**Architecture note:** the pivot (docs/00_V1_PIVOT.md) changed the *mechanism*
(Zig→Rust, HTML→TS, WebView→QuickJS, fixed→dynamic schema), not the *feature
ambitions*. Most v0.4 features are `neutral`/`port-needed` and carry into v1.

---

## Done — M0a substrate (built + tested)

| Area | Status | docs source | prd-merged | forge/ |
|---|---|---|---|---|
| Deterministic core vocabulary (errors, ids, envelopes) | done | 01, 06 | CR-A1/A4 | crates/domain |
| Canonical code hashing (provenance) | done | 21 | CR-9/010 | crates/domain (hash) |
| SQLite KV/oplog substrate + records projection | done | 27, 28 | DL-4 | crates/storage |
| Append-only CRDT chunk history | done | 21 | DL-6 | crates/storage |
| Loro CRDT records/text + convergence | done | 33 | DL-1/3/9 | crates/crdt |
| Patch vs replace (DL-9 field preservation) | done | — | DL-9 | crates/crdt |
| Dynamic schema registry, additive-only, stable field ids | done | 19, 27 | DL-7/8 | crates/schema |
| Capability + minimal RBAC engine | done | 07, 20 | SC-8/10 | crates/policy |
| QuickJS sandbox, zero ambient capability | done | 06, 07 | CR-1/2 | crates/runtime |
| Resource limits (cpu/mem/fuel/host-call/storage/log) | done | 22 | CR-5 | crates/runtime |
| Deterministic record/replay | done | 21 | CR-8/11 | crates/runtime |
| TS→JS transpile (SWC, in-core, offline) | done | — | CR-14 | crates/pipeline |
| Static policy scan (eval/Function/fetch/… reject) | done | 07 | CR-13/LM-9 | crates/pipeline |
| Engine-level eval/Function disable (2-layer) | done* | 07 | CR-13 | crates/runtime (WF-d) |

\* landing in the hardening workflow (WF-d-harden, reviews 005/006/009/010).

## In progress — finish the M0a spine (next: WF-C)

| Area | Status | docs | prd-merged | notes |
|---|---|---|---|---|
| Declarative UI component tree + diff/patch | planned | 23 | UI-1/2/6 | crates/ui — WF-C; UI-6 fallback is a blocker-class rule |
| Command/event facade (the spine wiring) | planned | 03, 31 | CR-A1/A2 | crates/core — WF-C |
| StorageHostBridge (ctx.db→SQLite write, ctx.ui→patch) | planned | 03 | CR-3/DL-4 | crates/core — WF-C |
| CLI harness `forge demo` + e2e proof | planned | 32 | PS-5 | crates/cli — WF-C; the M0a acceptance test |
| WASM lane (pure-logic crates wasm32-clean) | partial | — | CR-12/15 | domain/schema/policy/pipeline/runtime-trait wasm-clean; storage/runtime-engine native-gated; full QuickJS-WASM+SQLite-WASM backend = M0a-exit |

## Planned — data loop completion (post-spine, near-term)

| Area | Status | docs | prd-merged |
|---|---|---|---|
| Typed query DSL + live queries (`db.watch`) | planned | 03, 27 | DL-15/16 |
| Mutations: insert/update/patch/delete/transact | partial | 03 | DL-17 |
| Dynamic indexes (expression + FTS5) + lifecycle | planned | 27, 28 | DL-5 |
| Projection rebuild (`forge db rebuild`) | planned | 28 | DL-6 |
| Per-applet storage scope enforcement | partial | 20 | DL-18 |
| Workspace export/import (single SQLite file) | planned | 29 | DL-24 |
| Data migrations (additive, oplog ops) | planned | 19, 28 | DL-13 |
| Backup/export/import + restore | planned | 29 | DL-24 |

## Planned — runtime/security feature depth

| Area | Status | docs | prd-merged |
|---|---|---|---|
| Full ctx host API: net/files/secrets/notifications/clipboard/… | partial | 03, 20, 26 | CR-3 |
| Network egress policy (allowlist, DNS-pin, no raw fetch) | planned | 24 | SC-5 |
| App signing + trust (Ed25519, immutable installs) | planned | 17 | SC-15/MP-4 |
| App versioning + rollback (transactional installs) | planned | 18 | CR-7 |
| Snapshot/replay format (portable) | partial | 21 | CR-8/CR-9 |
| Resource budgets surfaced to review UI | partial | 22 | CR-5/UI-18 |
| Accessibility contract (WCAG AA, a11y primitives) | planned | 23 | UI-7 |
| Secrets via OS keychain (write-only refs) | planned | 07 | SC-13 |
| Audit log (all permission decisions) | partial | 07 | SC-12 |
| Permission review UX (resource-specific prompts) | planned | 03 | UI-18 |

## Planned — M0b / later milestones

| Area | Status | docs | prd-merged | milestone |
|---|---|---|---|---|
| JSC engine + cross-engine conformance suite | planned | 06 | CR-2/CR-12 | M0b |
| Full offline type-check (tsgo sidecar / tsc worker) | planned | — | CR-15 | M0b |
| RBAC v0 (customizable roles enforced) | partial | 07 | SC-11 | M0b |
| In-process client↔server sync (Loro vv exchange) | planned | 34 | SS-1/2 | M0b |
| Renderer zero (DOM) | planned | 23 | UI-13 | M0b |
| LLM pipeline (providers, context modes, repair loop) | planned | 11, 25 | LM-1..16 | M4 |
| Marketplace (source-visible installs, publisher auth) | planned | — | MP-3 | M4 |
| Home-server sync (cloud + embedded), migration | planned | 34 | SS-* | M2 |
| CRDT collab notebook | partial | 33 | DL-2 | M2 |
| Native shells (macOS→web→Windows→iOS→Android) | planned | 05, 26 | PS-* | M1+ |
| Reference-host conformance harness | planned | 32 | CR-12 | M0b |

## Dropped — legacy Zig/WebView-only (recorded, not built)

| Area | docs | why dropped |
|---|---|---|
| Zig event→action core + C FFI | 06 | replaced by Rust core |
| Native WebView host mounting (WKWebView/WebView2/GTK/WinRT) | 05 | replaced by QuickJS realms + thin renderers |
| Build-free HTML/CSS/vanilla-JS app packages | 04 | replaced by TS applets (SWC transpile) |
| Fixed bridge methods (`AppRuntime.call`) | 03 | replaced by typed `ctx` host API |
| Sandboxed iframe execution | 06 | replaced by QuickJS realm sandbox |
| Five hand-written native hosts vs reference-host oracle | 05, 32 | replaced by one Rust core + generated bindings |
| Codex platform MCP / control plugin (v0.4 shape) | 14, 16 | superseded by the Claude⇄Codex task board + v1 LLM pipeline |

---

## Implementation order (from synthesis + recommended_order)

1. **Finish the spine (WF-C):** ui → core → cli + e2e proof + WASM parity. *Spine right edge; the M0a executable acceptance test; UI-6 fallback is normative.*
2. **Data loop:** mutation/query path (DL-15/16/17), dynamic indexes (DL-5), projection rebuild (DL-6), export/import (DL-24).
3. **M0a gates:** crash torture (DL-23), perf budgets, full conformance scaffolding (CR-12).
4. **M0b:** JSC + conformance, offline type-check, RBAC v0, in-process sync, renderer zero.
5. **Feature depth then later milestones:** network policy, signing/trust, versioning/rollback, accessibility, secrets, audit; then M1+ (shells), M2 (sync/collab), M4 (LLM, marketplace).

## Codex delegation pipeline (from synthesis)

- **Fixtures + corpora:** workspace-compat fixtures, replay fixtures, hostile `*.ts`, injection `*.json`, conformance vectors — spec-driven + adversarial, no core Rust. (T001 ✓, T004 ✓ already in this vein.)
- **Spec extraction:** `@forge/std` full UI catalog (UI-2, 26 components), `ctx` type tables (CR-3), commands/errors/capabilities spec tables (CR-A2/A4, SC-8), UI golden-tree fixtures (UI-6). (T002 ✓ M0a subset; T005 files the UI golden-trees for WF-C.)
