# PRD 09 — Roadmap, Test Plan & Quality Gates

**Status:** Merged draft v1 · **Applies to:** all components
**Sources:** F-00 §10 (milestones) + P-12 (phase gates) + P-13 (test layers, fixtures, perf/reliability gates, release blockers) + decision D1 (e2e template first)

Production-first: each milestone exits only when its gates pass. No fixed calendar beyond the M0 estimate.

## 1. Milestones

### M0a — Executable spine (target 4–5 wks)
One proof, end-to-end, before anything else (Review 001):

```
TS source → SWC (in-core) → QuickJS-WASM → Rust capability ctx
→ SQLite write → UI tree patch → deterministic replay — all offline
```

Deliverables: crate skeleton (CR §2); command/event API spec v0; SQLite physical schema v0 + record envelope v1; minimal schema registry; Loro round-trip for one collection + one source file; `JsEngine` trait with the **QuickJS implementation** (rquickjs native + QuickJS-WASM); minimal capability engine (db/storage/ui/time/random); deterministic run recorder/replayer; SWC transpile + static policy scan; UI tree/patch/event wire format + golden-tree harness; CLI harness (PS-5) driving it all.
**Exit:** the spine demo runs headlessly in CI on macOS/Linux **and as a WASM target**; same run replays identically on both; kill-during-write torture passes.

### M0b — Conformance & template (target 4–5 wks)
Turn the spine into the platform template. Deliverables: **JSC engine implementation + cross-engine conformance suite** (CR-12 starts with the `conformance-engines` JS-language/determinism corpus; JSC remains an M0 commitment — it lands here, weeks after the spine, not at iOS time); full type-check stage (tsgo sidecar) + repair loop with mocked provider + LM Studio adapter; RBAC engine v0; dynamic indexes; in-process client↔server sync with partition simulation; **renderer zero** (UI-13); threat model doc; compatibility fixture framework.
**Exit:** full QuickJS loop (install → generate → run → store → sync → render-tree → event → patch) green on macOS/Linux/WASM; covered CR-12 vectors green on every wired engine; demo applet drives renderer zero; data survives schema change; prompt-to-run works offline with LM Studio.

### M1 — macOS alpha
SwiftUI shell + JSC + native renderer (conformance-kit validated), local-only, cloud + local LLM, editor/permission/review surfaces (UI §B).
**Exit:** 20 internal users build real applets; renderer + engine conformance green; cold start < 2 s.

### M2 — Sync beta
Managed cloud + embedded server GA-quality, workspaces, custom RBAC, presence, invites, file-level time travel, dynamic index rebuild UX.
**Exit:** 2-device + 2-user 7-day soak, zero divergence; desktop hosts a browser client; unauthorized sync op rejected + logged; restore-after-sync works.

### M3 — Web client
WASM core, OPFS (+ IndexedDB fallback), PWA, QuickJS-WASM workers, production web renderer, `tsc` worker type-check.
**Exit:** same workspace usable browser-only, offline after first load; web perf budget met; export/import roundtrip cross-platform.

### M4 — Local LLM + marketplace beta
In-core model manager; routing policy live; marketplace server beta (publisher auth, source-visible installs, mirror in self-host bundle).
**Exit:** family workspace runs zero-cloud; marketplace install→inspect→grant→run loop green incl. malicious-fixture corpus; eval-harness local-route targets met.

### M5 — GA
Billing, export/import GA + public file-format spec, support, docs, accessibility audit, security audit (SC-23), crash reporting opt-in, runbooks.
**Exit:** pen-test criticals fixed; SLOs met 30 days; all gates in §3–§6 green; previous-version fixture files open correctly.

### M6 — Windows → iOS → Android → SDK
Per PRD 06 §6–8; SDK/CLI private beta (harness productized).
**Exit:** per-platform conformance gates; App Store approval with 2.5.2 rationale; SDK builds a hand-written applet end-to-end.

## 2. Test layers (P-13, normative)

- **Unit:** domain validation; RBAC/capability decisions; schema compatibility; query planner; envelope encode/decode; manifest validation; host-API policy checks.
- **Integration:** workspace create/open/export/import; transpile → policy scan → sandbox run through the QuickJS spine plus covered engine vectors; SQLite transaction rollback; index rebuild/resume; Loro snapshot/chunk persistence; AI patch/test/fix loop with mocked provider; secret resolution with mocked keychain; client↔server sync in-process.
- **Property:** CRDT convergence under randomized edit order; unknown-field preservation through old-client writes; index rows ≡ canonical records; query results ≡ reference scan; **permission monotonicity**; deterministic replay identity.
- **Fuzz:** workspace file parser; envelope parser; query parser; host-call bridge; sync message parser; package manifest parser; **unknown-component UI fuzz** (UI-6).
- **Security:** sandbox-escape corpus; forbidden globals; prototype pollution; host-call argument fuzzing; SSRF/private-IP attempts; secret leakage scans; malicious package fixtures; malicious collaborator operations; injection corpus (LM-16).
- **Conformance (the template):** engine suite (CR-12: the `conformance-engines` corpus on QuickJS-native plus the adapter harness today; QuickJS-WASM/JSC added as wired engines); renderer kit (UI-14: golden trees + interactions + screenshots); cross-platform: same fixture opens everywhere, same deterministic run → same result, export A → import B.

## 3. Compatibility fixture suite (versioned forever, never deleted)

`workspace_v0_1.sqlite · workspace_v0_2.sqlite · workspace_future_unknown_fields.sqlite · schema_deprecated_field.sqlite · schema_new_index.sqlite · crdt_old_snapshot.sqlite · hard_purge_fixture.sqlite · marketplace_package_v1`
Every release: current app opens old fixtures; preserves future-unknown fields; old compatibility runner opens current file in expected degraded mode; export/import roundtrip.

## 4. Performance gates (budgets tracked in CI; regressions need explicit approval)

- Pipeline: SWC < 5 ms typical; type-check latency for 1/10/100-file applets per platform (incl. web worker); web type-checker artifact ≤ 10 MB gzipped, lazy-loaded outside the core budget (CR-15).
- Runtime: applet cold start < 150 ms desktop / < 400 ms web; host call p95 < 50 µs native / < 200 µs web; engine startup; memory baseline.
- Data: indexed query p95 < 10 ms desktop / < 50 ms web @100k records; SQLite write throughput; Loro merge throughput on large text; projection rebuild time.
- Sync: 1k-op catch-up p95 < 2 s; LAN p95 < 100 ms; cloud op round-trip p95 < 500 ms.
- App: cold start < 2 s desktop / < 3 s web TTI / < 2.5 s mobile; ≤ 6 MB gzipped web core; < 12 MB native core; workspace open for 10 MB / 100 MB / 1 GB fixtures.

## 5. Reliability gates

Power-loss simulation during writes (1,000 cycles, zero corruption); cancel runtime mid-host-call; kill app during sync and resume; partition + reconnect; duplicate/reordered peer messages; storage quota exceeded; OPFS unavailable fallback; keychain unavailable fallback; embedded-server restart drains cleanly.

## 6. Release blockers (any one blocks any release)

1. Sandbox escape or direct host-API bypass.
2. Unknown-field loss in compatibility tests.
3. Non-convergent CRDT test.
4. Workspace file corruption under simulated crash.
5. Secret leak into logs, sync payloads, or model context.
6. Engine or renderer divergence within the covered conformance suites.
7. iOS hidden code-execution path.
8. Marketplace package runnable before source/permissions visible.
9. Permission-monotonicity property failure.

## 7. Acceptance

- CI runs core + web suites on every commit; native smoke per platform before release; security corpus grows with every reported issue; performance dashboards per gate; fixture suite immutable and append-only.
