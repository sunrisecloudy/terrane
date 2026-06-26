# PRD 00 — Master Product Requirements (Merged)

**Codename:** Forge (brand TBD — Terrane brand assets exist and are a candidate)
**Status:** Merged draft v1 · **Date:** 2026-06-12 · **Owner:** Founder
**Sources:** `local_first_util_2/` ("Forge" pack, F) + `local_first_utility_prd_pack/` ("Utility" pack, P) + stakeholder decisions of 2026-06-12 (see `DECISIONS.md`)
**Sub-PRDs:** 01 Core Runtime · 02 Data Layer · 03 Sync & Server · 04 LLM System · 05 UI System · 06 Platform Shells · 07 Security · 08 Marketplace · 09 Roadmap & Quality Gates

---

## 1. One-liner

A local-first platform where an LLM (or the user) writes small personal apps ("applets") and automation scripts in TypeScript that run in a secure capability sandbox inside native shells on every device, with relational data on a dynamically evolvable schema that syncs and merges automatically across devices and collaborators — plus an optional centralized marketplace and cloud services that are never required for local use.

## 2. Problem

People have endless small software needs (trackers, dashboards, family tools, team utilities, one-off automations) that are too small for commercial apps and too technical to self-build. AI can now write this code, but there is no safe, portable, offline-capable place to *run* it: cloud app-builders require connectivity and trust, and raw generated code is unsafe to execute on personal devices.

## 3. Audience

| Persona | Priority | Need |
|---|---|---|
| **Maker / vibe coder** | Primary | Describe an app in plain language; get a working, private, synced app on all devices; invite family/teammates; guardrails, reversible changes, simple permissions. |
| **Developer** | Secondary | SDK access to the same runtime: hand-written applets, schema definitions, marketplace distribution, embedding. |

## 4. Core loop

1. User describes an app or change in natural language (or edits code directly).
2. LLM generates/updates a TypeScript applet + schema additions. Providers: cloud (BYOK or bundled) and local (LM Studio / in-core engine), with user-selectable context modes (PRD 04).
3. Pipeline — **fully offline-capable on every platform**: SWC transpile → TypeScript type-check → static policy scan → sandbox test run → auto-repair loop (≤ 3 iterations).
4. User reviews a plain-language summary, code diff, and **permission diff**; new grants always require explicit approval.
5. Applet installs atomically and runs in the shell; UI renders via the declarative component-tree protocol (PRD 05).
6. Data lives in the workspace data layer (Loro CRDT over SQLite with a dynamic relational schema); edits sync via the workspace's home server (cloud or embedded desktop server).
7. User iterates conversationally; applet code is itself CRDT text, so collaborative editing of apps works.

## 5. Product pillars

1. **Local-first.** Every core feature works offline with no account; sync is reconciliation, not a dependency; cloud services are optional and replaceable by self-hosting.
2. **Safe by construction.** Generated code runs only inside the capability sandbox; permissions are explicit, scoped, revocable; RBAC is enforced in the core, never only in UI.
3. **One core, native everywhere.** A single Rust core (engine, data, sync, LLM pipeline, policy) inside thin native shells per platform; the headless core + conformance suites *are* the template each new platform implements against.
4. **Forward compatible forever.** Old clients always open new data; schemas evolve additively with stable field IDs; unknown fields/components degrade gracefully, never crash, never get stripped.
5. **Collaboration is default.** Multi-device and multi-user real-time collaboration ship on the same CRDT machinery.
6. **Deterministic and inspectable.** Runs are recorded and replayable; all code (including marketplace installs) is source-visible and user-editable; audit logs cover every permission decision.

## 6. Strategy: end-to-end template first

The first deliverable is not a platform app — it is the **complete vertical slice running headless**: install → generate → sandbox-run → store → sync → emit UI trees, all exercised by Rust tests and a CLI harness that speaks the exact command/event API real shells use. A throwaway **renderer zero** (minimal DOM renderer, ~weeks in) validates the UI contract against real input/focus/scroll behavior while it is still cheap to change. Every later platform is "implement the renderer + platform services, pass the conformance kits": engine conformance (CR-12), renderer conformance (UI §3), data compatibility fixtures (DL/09), sync soak (SS §7).

## 7. Scope

### v1 (GA target)
- **macOS desktop app** (Swift/SwiftUI shell, JavaScriptCore engine) — first real shell.
- **Web app** (full client: Rust core in WASM, SQLite-WASM/OPFS, QuickJS-WASM workers, DOM renderer descended from renderer zero; installable PWA).
- **Linux headless build**: embedded-server CLI (same crate as desktop server mode).
- Managed cloud sync service **and** embedded self-hosted sync server inside the desktop app (LAN + relay).
- Hybrid LLM: cloud providers (BYOK + bundled metered), LM Studio adapter, downloadable in-core local model; per-project context modes (local-only / cloud-assisted / hybrid).
- Workspaces with customizable RBAC roles, real-time presence, invite links; **no login required for local use** — accounts exist only for cloud services.
- Curated TS stdlib (`@forge/std`); ~26-component declarative UI catalog; deterministic script runs with replay.
- Workspace export/import as a single SQLite-based portable file (public/open spec).
- **Marketplace (beta within v1 cycle):** centralized registry, publisher auth, source-visible editable installs, signing-ready format (PRD 08).

### v1.x (fast follow)
- **Windows desktop app** (WinUI 3/C# shell, QuickJS engine).
- iOS/iPadOS app (JavaScriptCore — held to the covered CR-12 engine vectors as it lands), then Android (QuickJS).
- Developer SDK + CLI (the M0 harness, productized).
- Marketplace GA: ratings, reviews, package signing.

### Non-goals (v1)
- Arbitrary npm imports at runtime (curated stdlib only).
- E2E encryption of synced data **by default** — the default is the server-readable workspace mode; an explicit opt-in **encrypted workspace mode** (project-level keys, server-side features disabled/degraded) is defined in SS-14, with shipping order decided by M2 demand.
- Native-code or WASM applets (TS only).
- Linux desktop GUI (headless server only).
- Peer-to-peer transport (home-server topology only; sync frames are transport-agnostic so a direct device pipe can be added later without protocol changes).
- Whole-workspace time rewind (file-level time travel ships in v1).

## 8. Architecture summary

```
┌─ Native shells (thin; no business logic) ───────────────────────────┐
│ macOS/iOS: Swift+SwiftUI · Windows: C#+WinUI3 (v1.x) ·              │
│ Android: Kotlin+Compose (v1.x) · Web: TS+DOM · Linux/server: CLI    │
├─ FFI: UniFFI (Swift/Kotlin/C#) · wasm-bindgen (web) — generated ────┤
│                    forge-core (Rust, single codebase)                │
│  • Command/Event/Stream API (versioned; the shell contract)         │
│  • JsEngine trait: JavaScriptCore (Apple) + QuickJS (rquickjs       │
│    native / QuickJS-WASM web) — covered-vector conformance gate     │
│  • Capability sandbox + RBAC policy engine + resource limits        │
│  • Data engine: Loro CRDT ⇄ SQLite KV/oplog + dynamic relational    │
│    schema + projection + FTS + dynamic indexes                      │
│  • Sync client (WebSocket, per-doc version vectors)                 │
│  • LLM pipeline (providers, SWC, offline type-check, policy scan,   │
│    repair loop, context modes)                                      │
│  • Deterministic run recorder/replayer + audit engine               │
│  • forge-server crate: cloud service AND embedded in desktop app    │
└─────────────────────────────────────────────────────────────────────┘
```

Crate layout, command catalog, and error model: PRD 01 §6 (adopted from P-04).

## 9. Monetization (open core)

- **Free:** local-only + self-hosted sync via own desktop, BYOK LLM, no account needed.
- **Pro ($/mo):** managed cloud sync, bundled LLM credits, web access to workspaces, relay for self-hosters, priority models.
- **Teams ($/seat):** roles/admin, audit log retention, SSO (post-v1).
- **Marketplace:** publisher services; paid packages post-v1.
- **Developers:** SDK free; commercial embedding licensed.
App Store builds route subscriptions through IAP; desktop direct sales avoid the 30% where possible.

## 10. Success metrics

| Metric | Target |
|---|---|
| Time from first launch → first working applet | p50 < 10 min |
| Generation pipeline pass rate (typecheck + tests, ≤ 3 repairs) | > 85% cloud · > 70% local route |
| Crash-free sessions | > 99.5% |
| Sync convergence after reconnect (1k ops) | p95 < 2 s |
| Offline task success (create + use applet, no network, local model) | 100% of supported task class |
| D30 retention (Makers with ≥ 1 applet) | > 35% |
| Deterministic replay fidelity (same inputs → same outputs, all platforms) | 100% |

## 11. Rollout

| Milestone | Contents | Exit criteria |
|---|---|---|
| **M0a** Executable spine (4–5 wks) | The jewel, headless: TS → SWC → QuickJS-WASM → Rust capability ctx → SQLite write → UI tree patch → deterministic replay, all offline; CLI harness speaking the real shell API | Spine demo green in CI on macOS/Linux **and WASM**; identical replay on both; kill-during-write torture passes |
| **M0b** Conformance & template (4–5 wks) | JSC engine + cross-engine conformance suite; full offline pipeline (type-check, repair loop, LM Studio); RBAC v0; in-process client↔server sync; **renderer zero** (DOM); fixture framework | QuickJS full loop (install→run→store→sync→render-tree→event→patch) green; covered CR-12 vectors green on every wired engine; renderer zero drives a demo applet; data survives schema change |
| **M1** macOS alpha | SwiftUI shell + JSC engine + native renderer (conformance-kit validated), local-only, cloud + LM Studio LLM, offline pipeline | 20 internal users build real applets; renderer conformance green |
| **M2** Sync beta | Embedded server GA-quality + managed cloud sync, workspaces, RBAC, presence, invites, file-level time travel | 2-device + 2-user 7-day soak, zero divergence; desktop hosts a browser client |
| **M3** Web client | WASM core, OPFS (IndexedDB fallback), PWA, QuickJS-WASM workers, web renderer (renderer zero lineage) | Same workspace usable browser-only, offline after first load |
| **M4** Local LLM + marketplace beta | In-core local model manager; marketplace server: publisher auth, source-visible installs, self-host mirror | Family workspace runs zero-cloud; package install→inspect→run loop works |
| **M5** GA | Billing, export/import GA, support, security audit, docs, public file-format spec | Pen-test passed (incl. sandbox escape + injection corpus); SLOs met 30 days; all 09 gates green |
| **M6** Windows → iOS → Android → SDK | Windows WinUI shell; iOS (JSC passes covered CR-12 vectors); Android; SDK/CLI beta | Renderer + engine conformance per platform; App Store approval |

## 12. Top risks

| Risk | Sev | Mitigation |
|---|---|---|
| iOS App Store rule 2.5.2 (downloaded code) | High | JavaScriptCore execution path (sanctioned) chosen from day one; user-created, source-visible/editable code framing; no public marketplace inside the iOS app at launch; review-safety mode documented (PRD 07 §9). |
| CRDT document growth / sync cost | High | Per-doc granularity + sharding, shallow snapshots, compaction policy, size budgets (PRD 02 §7). |
| LLM unit cost vs. consumer price | High | Local-model routing for small tasks; per-applet token budgets; BYOK tier (PRD 04 §6). |
| Dual-engine (JSC + QuickJS) divergence | Med-High | CR-12 starts with the JS-language/determinism corpus and grows host/API coverage as vectors land; divergence in covered vectors = release blocker. |
| Dynamic relational layer becomes a database project | Med | Narrow SQL/DSL subset, rebuildable indexes, compatibility fixtures, explicit non-goals (PRD 02). |
| Headless-first drifts from real UI needs | Med | Renderer zero by week ~3; golden-tree + interaction conformance kit; UI contract changes gated on renderer-zero validation (PRD 05). |
| Offline type-check weight on web/mobile | Med | SWC strip is always instant; full `tsc` lazy-loaded in a worker (web) / bundled (mobile); applets are small so check latency is bounded; measured as a perf gate (PRD 09). |
| Marketplace moderation/abuse before signing lands | Med | Source-visible installs, publisher auth, sandbox-only execution, abuse reporting; signing-ready format so enforcement can switch on without format break (PRD 08). |
| Two desktop shells strain a small team | Med | Windows deferred to v1.x; decision gate: if WinUI estimate exceeds budget, Tauri shell reusing the web renderer. |

## 13. Open questions

1. Brand name (Forge codename vs Terrane assets vs new).
2. Pro price point and bundled-credit sizing (needs M2/M4 cost data).
3. Web editor component: CodeMirror 6 vs Monaco.
4. Marketplace review requirements before first public beta; signing pulled into v1.x or v2.
5. Minimum OS versions (proposal: macOS 14+, iOS 17+, Windows 11/10 22H2+, Android 10+).
6. Whether `schedule` background tasks run with the shell closed on desktop (proposal: only while embedded server mode is on).
7. Hard-purge semantics detail for protected/sensitive records (PRD 02 §5 baseline adopted).
