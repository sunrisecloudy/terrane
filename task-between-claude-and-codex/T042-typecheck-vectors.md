---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/type-check.md, forge/fixtures/typecheck/*.json, forge/fixtures/typecheck/manifest.json
---

# T042 — CR-15 offline type-check diagnostics vectors

The audit's biggest CR gap: no offline TS type-check stage (tsgo) before install — type
errors are only caught at runtime. I want a spec + diagnostic vectors before the (heavy)
sidecar wiring, so the diagnostic contract is locked independent of the tsgo binary.

## Deliverables
1. `forge/spec/type-check.md` — derive from prd-merged/01 (CR-15/CR-16) and the existing
   SWC transpile pipeline (forge/crates/pipeline). Define: the type-check stage position
   (after parse, before/with transpile), the diagnostic shape (code, severity, message,
   file/line/col span), the pass/fail gate (install refused on a type error in deterministic
   mode), the @forge/std ambient types it checks against (forge/std/*.d.ts), and the bounded
   repair-loop input contract (a structured diagnostic list an LLM repair step can consume).
   Note the sidecar/version-pinning concerns as out-of-scope-for-the-spec but flag them.
2. `forge/fixtures/typecheck/<case>.json` + manifest. Each: a TS source (or applet) and the
   expected diagnostics (empty for clean) — engine-agnostic (assert the diagnostic contract,
   not a specific compiler's wording; use stable codes).

## Coverage (~12)
clean source -> no diagnostics; assigning string to number -> a type error with span;
calling ctx.db.insert with a wrong arg shape -> error against @forge/std types; missing
required manifest export (main) -> error; unknown identifier -> error; an unused-but-valid
construct -> no error (type-check, not lint); a correct generic usage -> clean; an await on a
non-promise -> error; an any-typed escape hatch -> clean (or a configurable warning); a
syntax error vs a type error are distinguished; multiple errors -> all reported with spans;
a valid use of a @forge/std UI component -> clean.

In `## Result`, flag the determinism/repro requirement (same source -> identical diagnostic
list, stable codes) so type-check fits the replay/repair model.

## Result
(codex fills this in)
