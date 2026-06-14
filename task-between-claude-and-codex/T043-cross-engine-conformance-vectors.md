---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/fixtures/conformance-engines/*.json, forge/fixtures/conformance-engines/manifest.json
---

# T043 — CR-12 cross-engine conformance expansion (JSC parity)

M0b exit + release-blocker-6 require cross-engine conformance, currently impossible with one
engine. Before JSC lands, expand the conformance corpus so a second engine (JavaScriptCore)
can be held to byte-identical behavior. Vectors only (the harness/JSC are Rust work).

## Deliverables
`forge/fixtures/conformance-engines/<case>.json` + manifest. Build on the existing
forge/fixtures/conformance/ corpus. Each case: a JS/TS snippet run via main(ctx, input) and
the expected deterministic output (return value + recorded host-call trace), chosen to expose
ENGINE DIVERGENCE risk areas where QuickJS and JSC historically differ.

## Coverage (~15) — divergence-prone areas
number formatting + float precision (toFixed, large ints, -0); Date determinism under the
seeded clock; JSON.stringify key ordering + escaping; string normalization / unicode + WTF-8
edges; Array sort stability; RegExp edge cases (unicode flag, lookbehind); Map/Set iteration
order; error .message / .stack shape (must be normalized); try/finally + async ordering /
microtask queue order; typed-array/ArrayBuffer behavior; BigInt; property enumeration order;
parseInt/parseFloat edge inputs; Math determinism (no Math.random in deterministic mode);
recursion/stack-limit behavior (must trip the limit identically, not diverge).

In `## Result`, flag which cases are the highest divergence risk and any that should be
normalized at the host boundary (e.g. error.stack) rather than required identical.

## Result
(codex fills this in)
