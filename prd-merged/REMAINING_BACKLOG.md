# Forge v1 — Remaining-features backlog (re-audit after feature-wave + /simplify, 2026-06-14)

Read-only re-survey (WF-reaudit) of prd-merged/docs PRD vs the current `forge/`.
**Landed since the original audit:** ctx.files (CR-3), UI event-dispatch loop (UI-4/CR-6),
renderer-zero (UI-13/14), applet lifecycle (CR-7, in progress), DL-19/21 compaction,
DL-24 export/import, the in-process sync kernel (SS-1/2) + SS-7 RBAC, and the full
/simplify refactor (workspace.rs facade + `commands/` registry, `host/` + storage modules).
LLM (LM-*) stays deferred to M4.

## Ranked remaining features (most valuable / unblocking first)

1. **[L] Live queries / db.watch (DL-16)** — SQLite update-hook dirty-set + watch_id registry +
   `db.watch`/`db.unwatch` commands + notification shape in RunRecord + dispatch into the event loop.
   *Highest leverage, ready NOW:* spec + **24 Codex fixtures done (T035 + T047)**, zero wiring. Makes
   the interactive loop reactive. Deps: UI-dispatch (landed). **← next feature.**
2. **[M] Audit-log persistence (SC-12)** — durable queryable `audit_log` + `audit.query` (by actor/
   action/decision/collection), append-only, redaction; fed by sync-RBAC + command-RBAC denials.
   Fixtures: **T048 e2e delivered; T031 semantic still `requested` (author first).**
3. **[M] SC-10 gate sources** — wire the 3 AllowAll stubs (workspace-policy, run-profile,
   platform-permission) to real trusted sources. Seam + DenyGate tests exist. Fixtures: **T037 `requested`.**
4. **[L] Schema migrations (DL-13)** — descriptor parsing + `Store.migration()` deterministic transforms
   (add-default/rename/drop/widen) + atomic rollback + oplog recording. **15 fixtures done (T033);
   spec `migrations.md` never delivered — author with impl.**
5. **[L] Time-travel / versioning + restore (DL-20)** — oplog history read API + per-record change feed +
   restore-as-new-version + 90-day retention. Pairs with audit-log. **No fixtures — needs a Codex task.**
6. **[M] Event-loop maturity (CR-6 rest)** — timers/schedule + work-stealing pool, deterministic ordering.
   Build after db.watch (shares plumbing). **No fixtures — needs a Codex task.**
7. **[S] MP-8 required_features negotiation** — manifest field + client feature registry + install refusal.
   Small, hardens install. Fixtures: **T038 `requested`.**
8. **[M] Quotas (DL-22)** — size caps + approaching-limit warnings + attachment dedup-by-hash. **No fixtures.**
9. **[XL] JSC engine + cross-engine conformance (CR-12)** — M0b release gate. Trait seam exists; no JSC backend.
   Fixtures: **T043 `requested`.**
10. **[XL] Type-check stage (CR-15)** — tsgo sidecar, deterministic diagnostics, install-gate. M0b gate +
    LLM-loop prereq. **No spec/fixtures — T042 `requested`.**

Beyond: SS-3..21 (transport/auth/server) and all LM-* are correctly later-milestone.

## Codex follow-ups (delegated but NOT yet delivered — needed for ranks 2-10)
T031 (audit-log semantic), T037 (policy-gates), T038 (required_features), T042 (type-check),
T043 (cross-engine conformance) are all still `requested`; T033's `migrations.md` spec was never
delivered. These feed ranks 2,3,4,7,9,10 — prioritize their delivery.

## Build order (driven by fixture-readiness + dependencies)
db.watch (1, ready) → migrations (4, ready) → audit-log (2, T031 needed) → SC-10 gates (3, T037 needed)
→ time-travel (5) / required_features (7) → quotas (8) / event-loop-rest (6) → JSC (9) → type-check (10).
