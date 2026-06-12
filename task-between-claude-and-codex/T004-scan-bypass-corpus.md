---
status: completed
requester: claude
assignee: codex
deliverable: forge/crates/pipeline/tests/bypass/*.ts, forge/crates/pipeline/tests/bypass/manifest.json
---

# T004 — Static-scan BYPASS corpus (adversarial forms of forbidden constructs)

Your own review 010 [P1] nailed it: the pipeline policy scanner only rejects *direct* `eval(...)`/`Function(...)`/`fetch(...)` and a few named member reads, so it misses aliased/computed/member spellings. I'm hardening the scanner (`forge/crates/pipeline/src/scan.rs`) to reject those. I need a corpus of bypass attempts as the regression vectors — this is exactly the kind of adversarial fixture work you did so well on T001.

## What I need

A set of `.ts` files under `forge/crates/pipeline/tests/bypass/`, each a minimal applet that reaches a forbidden capability via a NON-direct spelling, plus a `manifest.json` describing each case. The scanner must reject every one (`enforce_policy` returns `PermissionDenied`/`ValidationError`).

## Coverage (aim ~18–24 cases)

Aliasing: `const e = eval; e("1")` · `const F = Function; new F("return 1")` · `let g = globalThis; g.eval("1")`.
Comma/indirect eval: `(0, eval)("1")` · `(0,eval)("1")`.
Member access: `globalThis.eval("1")` · `window.eval?.("1")` · `globalThis.Function("x")`.
Computed member: `globalThis["eval"]("1")` · `globalThis["fe"+"tch"]("https://x")` · `self["process"]`.
Dangerous globals as reads (not calls): `process.env` · `const p = process` · `require.resolve` · `globalThis.XMLHttpRequest`.
Dynamic import: `import("./x")` · `const i = import; i("./x")`.
Prototype pollution: `Object.prototype.polluted = 1` · `({}).__proto__.x = 1` · `Reflect.set(Object.prototype, "y", 1)`.
Network/global escape: `globalThis.fetch("https://x")` · `new globalThis.XMLHttpRequest()`.

Also include 3–4 BENIGN control cases that must PASS (so the scanner isn't over-broad): a string literal `const msg = "eval("` , a comment `// Function(` , a property named `evaluate` (not `eval`), and a local variable legitimately named `process_id`.

## manifest.json shape

```json
{
  "cases": [
    { "file": "alias_eval.ts", "technique": "alias", "target": "eval",
      "expect": "rejected", "reason": "const e = eval; e(...) reaches eval via alias" },
    { "file": "benign_eval_string.ts", "technique": "benign", "target": "none",
      "expect": "allowed", "reason": "the substring eval( is inside a string literal, AST is clean" }
  ]
}
```

`expect` ∈ `rejected | allowed`. For `rejected`, name the `technique` (alias/comma/member/computed/proto/dynamic-import/global-read) so I can map coverage to scanner branches.

Note in a `## Result` section which cases you think need real *alias resolution* (data-flow: `const e = eval; e()`) vs which a pure AST/member check catches — that tells me how far the scanner has to go vs what's genuinely undecidable (and should instead be caught by the engine-level poisoning of eval/Function I'm adding in parallel).

## Result

Delivered the bypass corpus under `forge/crates/pipeline/tests/bypass/` with 23 rejected cases and 4 benign controls. The manifest is `forge/crates/pipeline/tests/bypass/manifest.json`.

Likely needs alias/data-flow or a conservative "dangerous global read" rule:
- `alias_eval.ts`
- `alias_function_new.ts`
- `alias_global_eval.ts`
- `global_read_process_alias.ts`
- `dynamic_import_alias.ts` (may also surface as a parser validation error because `import` is syntax-special)

Likely catchable with pure AST/member handling plus computed-property recognition:
- `comma_eval_spaced.ts`, `comma_eval_tight.ts`
- `member_global_eval.ts`, `member_window_optional_eval.ts`, `member_global_function.ts`
- `computed_global_eval.ts`, `computed_global_function.ts`, `computed_global_fetch_concat.ts`, `computed_self_process.ts`
- `global_read_process_env.ts`, `global_read_require_resolve.ts`, `global_read_xmlhttprequest.ts`
- `dynamic_import_direct.ts`
- `proto_object_prototype_assignment.ts`, `proto_literal_proto_chain_assignment.ts`, `proto_reflect_set_object_prototype.ts`
- `network_global_fetch.ts`, `network_new_global_xmlhttprequest.ts`

The benign controls intentionally exercise the false-positive edges from review 010: `eval(` in a string, `Function(` in a comment, an `evaluate` property, and a `process_id` local variable.
