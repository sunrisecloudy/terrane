# Feature Coverage Map — docs/ → forge v1

**Purpose:** the durable backlog for the goal *"implement every feature in the
original `docs/` PRD."* Built from a fan-out extraction of all 37 `docs/` specs
(832 atomic requirements) cross-referenced against `prd-merged/` (the v1 plan)
and the built `forge/` crates. Organized by **feature area** (more actionable
than atomic rows), each with status, source docs, the governing `prd-merged`
requirement, and the `forge/` location.

**Last refreshed:** 2026-06-13 — reflects state through the in-process CRDT sync
milestone (SS-1/2, forge-sync crate committed, convergence fixtures green).

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
| Engine-level eval/Function disable (2-layer) | done | 07 | CR-13 | crates/runtime |
| Command/event facade + spine wiring (WorkspaceCore) | done | 03, 31 | CR-A1/A2 | crates/core |
| StorageHostBridge (ctx.db→SQLite write, ctx.ui→patch) | done | 03 | CR-3/DL-4 | crates/core |
| CLI harness `forge demo` + e2e proof | done | 32 | PS-5 | crates/cli |

## Done — data layer completion (post-spine)

| Area | Status | docs source | prd-merged | forge/ |
|---|---|---|---|---|
| CRDT-backed record writes + projection rebuild (source of truth) | done | 27, 28 | DL-4/DL-6 | crates/storage (crdt_write), crates/core (bridge) |
| Typed query DSL + planner + aggregates | done | 03, 27 | DL-5/DL-15 | crates/storage (query) |
| Dynamic indexes (expression + FTS5) + lifecycle management | done | 27, 28 | DL-5 | crates/storage (index) |
| `ctx.db.query` applet-facing host call | done | 03, 27 | DL-15 | crates/core (bridge) |
| Workspace export/import (portable single-file SQLite bundle) | done | 29 | DL-24 | crates/storage (export), crates/core (workspace) |
| `schema.apply_change` / `validate_compatibility` / `rebuild_indexes` commands | done | 19, 27 | DL-7/DL-8 | crates/schema, crates/core (workspace) |
| Per-applet storage scope enforcement (KV namespaces) | partial | 20 | DL-18 | crates/storage (KV namespace key); applet-ID enforcement wired in bridge |

## Done — runtime/security feature depth

| Area | Status | docs source | prd-merged | forge/ |
|---|---|---|---|---|
| Network egress policy (allowlist, DNS-pin, private-network deny) | done | 24 | SC-5 | crates/policy (net, net_url) |
| `ctx.net.fetch` applet host call + injectable HttpClient seam | done | 03, 24 | CR-3/SC-5 | crates/runtime (net), crates/core (bridge) |
| `ctx.secrets` injection (write-only refs, trace-safe, never logged/synced) | done | 07 | SC-13 | crates/secrets |
| App signing + trust, Ed25519 verify (canonical preimage, 3-layer failure) | done | 17 | SC-15/MP-4 | crates/signing |
| Signed manifest bound to enforced install payload | done | 17 | SC-15 | crates/signing, crates/core |

## Done — sync milestone (in-process, SS-1/2)

| Area | Status | docs source | prd-merged | forge/ |
|---|---|---|---|---|
| In-process CRDT chunk-diff sync (both-direction, content-addressed frontier) | done | 34 | SS-1/SS-2 | crates/sync |
| Sync convergence fixtures (10 canonical scenarios, idempotence proofs) | done | 34 | SS-2 | forge/fixtures/sync/ |
| Sync protocol spec | done | — | SS-1/SS-2 | forge/spec/sync-protocol.md |

## Done — FFI / platform binding

| Area | Status | docs source | prd-merged | forge/ |
|---|---|---|---|---|
| C ABI thin layer (forge-ffi, panic catch, JSON envelope, opaque handle) | done | 05, 32 | PS-* | crates/ffi |
| Windows C# shell skeleton (WinUI3, Forge.Core, Forge.Windows.sln) | partial | 05 | PS-* | windows/ (remote team) |

---

## Planned — M0b gates (not yet built)

| Area | Status | docs | prd-merged | milestone |
|---|---|---|---|---|
| JSC engine + cross-engine conformance suite | planned | 06 | CR-2/CR-12 | M0b |
| Full offline type-check (tsgo sidecar / tsc worker) | planned | — | CR-15 | M0b |
| RBAC v0 (customizable roles enforced) | partial | 07 | SC-11 | M0b |
| Renderer zero (minimal DOM renderer, UI contract validation) | planned | 23 | UI-13 | M0b |
| Reference-host conformance harness | planned | 32 | CR-12 | M0b |
| Declarative UI component tree + diff/patch (forge-ui spine wiring) | partial | 23 | UI-1/2/6 | M0b; crates/ui substrate exists, full wiring planned |

## Planned — feature depth (post-M0b)

| Area | Status | docs | prd-merged | notes |
|---|---|---|---|---|
| `ctx.files` host API | planned | 03, 26 | CR-3 | not started |
| Accessibility contract (WCAG AA, a11y primitives) | planned | 23 | UI-7 | not started |
| Permission review UX (resource-specific prompts) | planned | 03 | UI-18 | not started |
| App versioning + rollback (transactional installs) | planned | 18 | CR-7 | not started |
| Audit log (all permission decisions) | partial | 07 | SC-12 | policy engine makes decisions; audit persistence not yet wired |
| Data migrations (additive, oplog ops) | planned | 19, 28 | DL-13 | not started |
| Snapshot/replay format (portable, cross-platform) | partial | 21 | CR-8/CR-9 | replay works in-process; portable cross-engine format deferred |
| Resource budgets surfaced to review UI | partial | 22 | CR-5/UI-18 | budgets enforced; UI surface planned |
| WebSocket sync transport + server-side RBAC | planned | 34 | SS-7 | SS-1/2 done in-process; network transport deferred to M2 |
| Home-server sync (cloud + embedded), migration | planned | 34 | SS-* | M2 |
| CRDT collab notebook | partial | 33 | DL-2 | CRDT machinery done; notebook product layer planned |
| WASM lane (QuickJS-WASM + SQLite-WASM backend) | partial | — | CR-12/15 | domain/schema/policy/pipeline wasm-clean; storage/runtime native-gated; full WASM = M0a-exit gate |

## Planned — later milestones (M1+)

| Area | Status | docs | prd-merged | milestone |
|---|---|---|---|---|
| macOS SwiftUI shell + JSC engine (native renderer) | planned | 05, 26 | PS-* | M1 |
| LLM pipeline (providers, context modes, repair loop) | planned | 11, 25 | LM-1..16 | M4 |
| Marketplace (source-visible installs, publisher auth, signing enforcement) | planned | — | MP-3 | M4 |
| Windows WinUI3 shell (C# skeleton exists — remote team) | partial | 05 | PS-* | M6 |
| iOS/Android native shells | planned | 05 | PS-* | M6 |

---

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

## Implementation order (updated post-sync milestone)

1. **M0b gates:** JSC engine + conformance suite (CR-12), renderer zero (UI-13), full offline type-check (CR-15), RBAC v0 (SC-11). WASM lane completion.
2. **Feature depth:** `ctx.files`, accessibility wiring (UI-7), audit log persistence (SC-12), app versioning/rollback (CR-7), data migrations (DL-13).
3. **M1:** macOS SwiftUI shell + JSC engine (native renderer), cloud + LM Studio LLM.
4. **M2:** WebSocket sync transport, embedded server GA, workspaces, RBAC, presence, invites, file-level time travel.
5. **M3 → M4 → M5 → M6:** Web WASM client; local LLM + marketplace beta; billing/GA; Windows/iOS/Android shells + SDK.

## Codex delegation pipeline (from synthesis)

- **Fixtures + corpora:** workspace-compat fixtures, replay fixtures, hostile `*.ts`, injection `*.json`, conformance vectors — spec-driven + adversarial, no core Rust. (T001 ✓, T004 ✓, T026 ✓ already in this vein.)
- **Spec extraction:** `@forge/std` full UI catalog (UI-2, 26 components), `ctx` type tables (CR-3), commands/errors/capabilities spec tables (CR-A2/A4, SC-8), UI golden-tree fixtures (UI-6). (T002 ✓ M0a subset; T005 files the UI golden-trees.)
- **Sync fixtures:** in-process convergence scenarios (T026 ✓ — 11 scenarios, idempotence + LWW + delete propagation).
