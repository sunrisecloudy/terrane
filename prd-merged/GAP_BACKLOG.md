# Forge v1 — Verified Gap Backlog (audit 2026-06-14)

Evidence-based completeness audit (WF-AUDIT): one reader per prd-merged spec doc,
each requirement classified implemented | partial | missing against the actual
crates with file evidence, then synthesized into a prioritized backlog. This is a
snapshot to drive feature sequencing; it supersedes guesswork, not FEATURE_COVERAGE.md.

## State of the system

The M0a executable spine is solid and proven end-to-end: TS→SWC→QuickJS→capability-checked
ctx→CRDT(Loro)+oplog+SQLite projection in one transaction→UI tree/patch→deterministic
byte-identical replay, CLI-driven over 277+ fixtures. **Data layer is the strongest area**
(DL-4 atomic writes, DL-6 rebuild, DL-5/15 query+indexes, DL-24 export/import, DL-9/10
forward-compat). **Security substrate is real and tested** (CR-1/SC-1 zero-ambient, CR-5
limits, SC-5 egress, SC-13 secrets, SC-15 signing, SS-7 sync RBAC). **The frontier is
everything that turns the spine into a usable app.**

## Coverage by area (impl / partial / missing)

| Area | impl | partial | missing | Note |
|---|---|---|---|---|
| Core runtime (CR) | 13 | 12 | 2 | Spine well-tested; gaps: CR-6 event loop, CR-7 lifecycle, CR-12 JSC, CR-15 type-check |
| Data layer (DL) | 19 | 4 | 2 | Strongest; missing DL-16 watch, DL-19 compaction, DL-20 time-travel, DL-25 encryption |
| Sync (SS) | 6 | 6 | 10 | In-process kernel proven; all network/server/auth missing |
| LLM (LM) | 0 | 1 | 23 | Deferred to M4 (correct) |
| UI (UI) | 6 | 7 | 8 | Protocol solid; **UI-4 events serialize but never dispatch**; UI-2 5/26 catalog; UI-13 no renderer |
| Platform shells (PS) | 1 | 7 | 11 | CLI harness (shell zero) only in-scope shell; native shells remote-team, early |
| Security (SC) | 10 | 8 | 7 | Capability engine real; SC-10 3/7 gates AllowAll stubs; SC-12 audit sync-only |
| Marketplace (MP) | 2 | 7 | 3 | Signing/install only; MP-8 required_features negotiation missing |
| Roadmap/gates (09) | 58 | 30 | 10 | M0a proven; M0b partial (no JSC/WASM/renderer/tsgo); **no CI .yml in repo** |

## Prioritized backlog

1. **[M] ctx.files capability** (CR-3/SC-1/SC-8/9) — fixtures DONE (T028, 13 vectors) but
   ZERO runtime wiring. Same precedent as net/secrets. No deps. Highest leverage ready now.
2. **[L] UI event dispatch loop** (UI-4/CR-6) — the keystone: route serialized onTap/onChange
   ActionRefs back into the applet, re-enter the engine → next UI patch. VERIFIED missing.
   Unblocks renderer, lifecycle, live queries. No structural deps.
3. **[L] Renderer-zero** (UI-13/14) — minimal reference TS renderer consuming the UI tree/patch
   wire format; second consumer required by M0a exit + release-blocker-6. Static trees land
   without #2; interactive needs #2.
4. **[L] Applet lifecycle** (CR-7) — enable/run-long-lived/suspend/atomic-upgrade/uninstall.
   Only install exists. Depends on #2.
5. **[M] Live queries / db.watch** (DL-16) — SQLite update hooks + dirty-set + async notify.
   Reactive complement to #2. Depends on #2.
6. **[M] Audit log persistence** (SC-12/DL-20) — durable queryable audit beyond sync-op. T031 requested.
7. **[M] Time-travel / versioning + rollback** (DL-20/CR-7) — expose oplog/chunk history; restore-as-new-version. T032 requested.
8. **[M] Wire SC-10 gate stubs** (SC-10) — replace 3 AllowAll gates with enforced sources. Seams exist.
9. **[XL] JSC engine + cross-engine conformance** (CR-12/CR-2) — M0b exit + release-blocker-6. Fixtures ready (11 vectors).
10. **[XL] Type-check stage (tsgo sidecar)** (CR-15/16) — biggest CR gap; M0b gate; prereq for LLM loop.

## Risks (verbatim from audit)

- **"Looks done but isn't":** ctx.files marked done as a Codex task with full fixtures but ZERO
  runtime wiring — confirm a code path, never just a fixture dir.
- **The interactive loop is a hidden hard dependency:** UI-4 reported partial because ActionRefs
  serialize, but nothing dispatches them. Ranks 3/4/5 silently depend on rank 2.
- **Release-blockers 4 (crash-corruption torture) and 6 (cross-engine divergence) are structurally
  unsatisfiable today** (single engine, no power-loss harness).
- **No CI enforced in-repo** (no .yml, no perf/reliability gate) — every PRD09 gate is advisory.
- **Native shells diverge from spec engines** (WKWebView/WebView2/WebKitGTK vs JSC/QuickJS/WinUI);
  late JSC-conformance divergence could force shell rework.
- **Sync is a verified kernel, not a usable multi-device feature** (no transport/auth/invite).
- **Permission-monotonicity (SS-9) asserted via example vectors, not a property test.**
