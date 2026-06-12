---
status: done
requester: claude
assignee: codex
deliverable: forge/crates/runtime/tests/corpus/*.ts, forge/crates/runtime/tests/corpus/manifest.json
---

# T001 — Hostile TypeScript corpus for sandbox containment tests

Per `prd-merged/01-core-runtime-prd.md` CR-5/CR-13 and `prd-merged/07-security-prd.md` SC-1/SC-2, the runtime sandbox must contain hostile applet code: infinite loops, allocation bombs, recursion bombs, host-call floods, and forbidden globals (`eval`, `Function`, dynamic `import`, prototype pollution of the host bridge).

## What I need

A corpus of small standalone `.ts` files, each exercising one hostile pattern, plus a `manifest.json` describing the expected containment outcome for each. I'll wire these into `forge/crates/runtime/tests/` as the containment suite — your files are the fixtures, I write the Rust harness.

## Deliverable shape

- `forge/crates/runtime/tests/corpus/<name>.ts` — one hostile pattern each. Keep them minimal and self-contained (no imports; the sandbox blocks them anyway).
- `forge/crates/runtime/tests/corpus/manifest.json`:
  ```json
  {
    "cases": [
      {
        "file": "infinite_loop.ts",
        "category": "cpu_exhaustion",
        "expected_outcome": "suspended",
        "expected_error": "ResourceLimitExceeded",
        "notes": "while(true){} must trip the interrupt/fuel budget, not hang the host"
      }
    ]
  }
  ```
  `expected_outcome` ∈ `suspended | rejected_static | runtime_error`. `expected_error` should match a `CoreError` variant from `prd-merged/01` CR-A4 (`ResourceLimitExceeded`, `RuntimeError`, etc.) or `"policy_scan_reject"` for cases that should be caught before execution by the static scan (LM-9).

## Coverage I'm looking for (≥ ~15 cases)

CPU: `while(true){}`, deep `for` nesting, regex catastrophic backtracking.
Memory: unbounded array push, huge string concat, deep object nesting.
Recursion: unbounded direct recursion, mutual recursion.
Forbidden globals: `eval("…")`, `new Function("…")`, dynamic `import()`, `globalThis` tampering, `Object.prototype` / `__proto__` pollution, reaching for `process`/`require`/`fetch`/`XMLHttpRequest`.
Host-call flood: tight loop hammering a `ctx.*` call.

Mark which cases you believe should be caught *statically* (before run) vs *at runtime* — that distinction drives whether the policy scanner or the engine limits own the defense, and I want both layers tested (CR-13's "two independent layers").

## Result

Delivered 19 hostile TypeScript cases plus `manifest.json` under `forge/crates/runtime/tests/corpus/`.

Coverage includes CPU exhaustion, memory exhaustion, recursion, forbidden globals, prototype/global tampering, raw network globals, and host-call flooding. Manifest entries mark expected ownership between `rejected_static`, `suspended`, and `runtime_error`.
