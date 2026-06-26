---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: task-between-claude-and-codex/codex-response-T049.md (a drafted M0b final report)
---

# T049 — Draft the M0b final report (scope frozen)

The user has FROZEN scope at current M0b and asked to skip the two remaining XL
items (a real JavaScriptCore/JSC engine backend for CR-12, and the tsgo type-check
sidecar for CR-15) and move to the wrap-up: WASM target check + e2e demo + final
report. Please draft the final report so Claude can refine + finalize it.

## What to produce (`codex-response-T049.md`)
A concise, accurate M0b status report with these sections:

1. **Master invariant status** — state that the acceptance gate is
   `cargo test --workspace` green + `cargo clippy --workspace --all-targets -D warnings`
   clean + `cargo run -p forge-cli -- demo` prints `REPLAY IDENTICAL: true`. (Claude
   will paste the live demo output.)

2. **Feature-completeness matrix** — walk `prd-merged/` and list each v1/M0b feature
   id (CR-*, DL-*, SC-*, SS-*, UI-*, MP-*) with: implemented? (yes/partial/deferred),
   the crate(s)/command(s) that realize it, and the conformance fixtures that cover it.
   Base this on the actual code + `forge/fixtures/` + `forge/spec/`, not assumptions.

3. **Review-closure ledger** — summarize `review/` (Codex's independent reviews):
   how many reviews, that every P1/P2 was closed (note the latest open one, 183, is
   being fixed now; 182 P2 is deferred-under-freeze as a known minor follow-up).

4. **Explicitly deferred (out of frozen scope)** — with one-line rationale each:
   - CR-12 real JSC backend (needs a system JavaScriptCore framework; the cross-engine
     conformance framework + corpus + determinism hardening ARE done and engine-agnostic
     via the `JsEngine` trait, ready to hold JSC later). See `forge/spec/cross-engine-conformance.md`.
   - CR-15 type-check tsgo sidecar (heavy external `tsgo` binary dependency).
   - Any other partial items you find (e.g. multi-collection atomic transact across the
     sync boundary, real-disk ctx.files TOCTOU hardening) — list honestly.

5. **WASM target** — note that Claude is running a wasm32 build-feasibility check in
   parallel; leave a short placeholder subsection for the findings.

Keep it factual and cite files. Do NOT edit any source; this is a drafted report only.

## Result
(codex fills this in)
