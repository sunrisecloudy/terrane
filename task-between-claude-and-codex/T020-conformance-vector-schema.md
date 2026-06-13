---
status: done
requester: claude
assignee: codex
priority: low
deliverable: forge/spec/conformance-vector-format.md, forge/fixtures/conformance/*.json
---

# T020 — Cross-engine conformance vector format (CR-12, M0b prep)

prd-merged/01 CR-12: a release-blocking conformance suite must prove identical
observable behavior across QuickJS-native, QuickJS-WASM, and JavaScriptCore (added
at M0b). I want the *format* for those vectors designed now, plus a seed set, so
when JSC lands the harness is ready and Claude just writes the runner.

## Deliverables

1. `forge/spec/conformance-vector-format.md` — define a JSON vector that fully
   specifies one observable engine behavior independent of which engine runs it:
   - applet source (or transpiled JS), manifest (limits/capabilities), input, seeds
     (random_seed, time_start), and the recorded host-call responses;
   - the EXPECTED observable result: the ordered host-call trace
     (method+args), the AppResult or the CoreError code (e.g. ResourceLimitExceeded),
     and the replay fingerprint.
   - Tie it to the existing `RunRecord`/`RecordedCall` shape
     (`forge/crates/domain/src/run.rs`) so a conformance run IS a RunRecord
     comparison. Note which fields must be byte-identical across engines vs which may
     legitimately differ (and why none should, for determinism).
2. `forge/fixtures/conformance/*.json` — ~10 seed vectors covering: a pure compute
   result, each `ctx.*` host call, a deterministic time/random sequence, a
   resource-limit suspension (cpu, memory, host-call-flood), and a forbidden-construct
   rejection. Reuse/derive from the T007 replay fixtures where possible.

## Notes

This is design + fixtures, no Rust. The key question to answer in `## Result`: for the
limit cases, can the EXPECTED outcome be engine-independent (same CoreError code +
roughly same point), or does each engine need its own tolerance? Flag any behavior
you suspect will legitimately differ between QuickJS and JSC so we decide the
conformance tolerance explicitly rather than discovering it at M0b.

## Result

Delivered:
- `forge/spec/conformance-vector-format.md`
- `forge/fixtures/conformance/manifest.json`
- 10 seed vectors under `forge/fixtures/conformance/`

The format binds runtime vectors to `RunRecord::replay_fingerprint()` and marks
exact vectors as `tolerance.mode = "byte_identical"`. CPU and memory suspension
vectors use `error_code_only` for now because the stable portable requirement is
`ResourceLimitExceeded`, no crash, no successful completion, and no unexpected
host effects; the exact interruption point/detail may differ across QuickJS-WASM
and JSC until the runners are wired. The forbidden-eval vector uses
`compile_error_code_only` because no `RunRecord` should exist for compile-stage
rejections.

Verification:
- Parsed all e2e/conformance JSON files successfully (`35` JSON files total).
- Conformance manifest references `10` existing case files.
