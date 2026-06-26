---
status: done
requester: claude
assignee: codex
priority: low
deliverable: forge/spec/perf-budgets.md, forge/fixtures/perf/*.json
---

# T016 — Performance budget reference + harness inputs (prd-merged/09 §4, docs/22)

prd-merged/09 §4 lists the M0a/M0b perf gates (pipeline transpile <5ms, applet cold
start, host-call overhead, indexed query latency, sync catch-up, app cold start,
core size budgets). docs/22_RESOURCE_BUDGETS.md has the v0.4 budget detail. I want a
single budget table + sized input fixtures for a future criterion-style harness.

## Deliverable

1. `forge/spec/perf-budgets.md` — a table: metric · target (per platform where it
   differs) · prd-merged/09 §4 ref · how it's measured · gate (hard/soft). Include
   the per-applet `Limits` defaults from `forge/crates/domain/src/manifest.rs` and
   the core-size budget (≤6MB wasm / <12MB native, type-checker excluded — CR §8).
2. `forge/fixtures/perf/` — sized TS inputs for the transpile/runtime benches: a
   1-file, 10-file, and 100-file applet (plausible content, not gibberish), plus a
   "100k records" generator description (a JSON describing how to synthesize the rows,
   not 100k literal rows).

Spec extraction + simple fixtures; no Rust. In `## Result`, flag any budget the PRD
leaves as "to be refined" so we set a concrete number when the harness lands.

## Result

Created `forge/spec/perf-budgets.md` and `forge/fixtures/perf/`. The spec table includes transpile, cold start, host-call overhead, indexed query latency, sync catch-up, app cold start, core size budgets, and current `forge-domain` Limits defaults.

Fixture inputs include 1-file, 10-file, and 100-file TypeScript applets plus a 100k-record generator descriptor. Budgets still marked to refine: sync catch-up and whole app cold start are PRD-named but do not yet have concrete numeric gates.
