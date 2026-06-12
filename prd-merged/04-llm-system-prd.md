# PRD 04 — LLM Coding System (hybrid cloud + local, user-controlled)

**Status:** Merged draft v1 · **Depends on:** 01, 02 · **Depended on by:** 05, 06
**Sources:** F-04 (pipeline, routing, eval harness, injection defenses, budgets) + P-08 (context modes, AI modes, LM Studio, generation contract, versioning) + decision D7 (fully offline pipeline)

## 1. Purpose

The system that turns natural language into installed, working applets and scripts: provider abstraction (cloud + local), permissioned context assembly, TypeScript generation, the verify-and-repair loop, review/permission UX, and cost governance. TypeScript is the target because typed output gives the pipeline a mechanical error signal for self-repair. The LLM is the primary code generator, but it **cannot bypass type-check, policy scan, tests, RBAC, capability prompts, or versioning** — ever.

## 2. Providers

- **LM-1** `LlmProvider` trait: chat completion with tool calls, streaming, token accounting. v1 backends: Anthropic API, OpenAI-compatible endpoint (covers BYOK aggregators, Google via compat, **LM Studio at `localhost`**), and the in-core local engine.
- **LM-2** Key modes: **bundled** (metered credits on Pro), **BYOK** (OS keychain, never synced, never logged, never in model context). Provider configs are local-first records with `secret_ref`s (P-08).
- **LM-3** Local engines: (a) **LM Studio adapter** — zero-install path for users who already run it (M0/M1); (b) **in-core engine** (`mistral.rs`/llama.cpp-class) with a shell Model Manager (download, hash-verify, delete, disk/RAM disclosure) — M4. Web/mobile route local-tier requests to the user's embedded home server when it has a model; else cloud; never silently.

## 3. Context modes (P-08; per-workspace, user-selectable)

- **LM-4** **Local-only:** no project content leaves the device; cloud generation unavailable. **Cloud-assisted:** user-approved context to configured cloud provider. **Hybrid:** local model indexes/summarizes; user approves the selected context sent to cloud.
- **LM-5** Context builder obeys permissions and is deterministic/cached: `@forge/std` type surface, schema registry, target applet sources, UI component catalog, house rules, selected files/logs/errors, ≤ 20 PII-masked sample rows (maskable off for BYOK). Secrets never; logs redacted by default; app data only with explicit permission. Hard token cap with priority-ordered truncation; assembly is pure-function testable.

## 4. AI modes (P-08; per-workspace setting + per-request override)

- **LM-6** **Suggest-only** (explanation + patch, manual apply) → **Assisted apply** (pipeline verifies, user approves) → **Auto within sandbox** (applies + tests + fixes automatically; user approval before first real execution unless trusted) → **Full auto for trusted workspace** (iterate until tests pass within budgets; destructive actions still confirm). All modes produce versioned, reversible changes.

## 5. Generation pipeline (normative; offline-capable end-to-end)

```
intent → context pack → generate (stream) → SWC transpile (in-core)
→ TS type-check (offline: tsgo sidecar / web worker tsc / bundled mobile — CR-15)
→ static policy scan → sandbox test run (deterministic clock/RNG, db fixture)
→ [errors? → repair loop ≤ 3 (or auto-mode budget)]
→ human review: summary + code diff + permission diff → atomic install (CR-7)
```

- **LM-7** Generation contract (P-08): the model must produce file list, patch/diff, manifest changes, schema changes, tests, plain-language explanation, **permission rationale**, and rollback note — as a validated structured object.
- **LM-8** Verification: type-check green required to install; tests green required for autonomous apply (user-overridable with warning). Tests run in the real sandbox with injected clock/RNG and synthetic fixtures.
- **LM-9** Static policy scan (defense-in-depth; engine already blocks at runtime): reject `eval`/`Function`/dynamic import, raw fetch of non-manifest domains, secret-exfil patterns (secrets → net payload requires extra confirm), unbounded-recursion heuristics.
- **LM-10** Repair loop: compiler/test errors + failing snippets fed back; default ≤ 3 iterations; auto modes use budgets (max iterations, provider cost, wall time, file changes, runtime runs, context bytes) with hard stop + clear UI (P-08).
- **LM-11** Review UX: plain-language change summary, expandable diff, and a **permission diff** ("wants NEW access: network → api.weather.com"). New permissions always require explicit grant; no silent expansion.
- **LM-12** Routing policy (user-overridable): local tier for offline/small edits/formatting/schema additions/test generation; cloud for new applets/multi-file changes/error debugging. If offline and local can't handle it: honest failure, never silent degradation.

## 6. Versioning & provenance (P-08)

- **LM-13** Every AI action records: prompt hash, model/provider metadata, context manifest, generated patch, applied diff, test results, run IDs, user approval decision. Local-first; syncs only where the user permits chat/history sync (DL-2).

## 7. Prompt injection & safety invariants (F-04 + P-08; cross-ref PRD 07 §3)

- **LM-14** Invariant: **generation can propose; only human review can grant.** The LLM cannot grant permissions to its own code, cannot read files outside the permitted context builder, cannot access secrets, cannot bypass pipeline steps, and its output is always source-visible/editable.
- **LM-15** Synced data entering context is framed as inert quoted data ("data, not instructions"); high-risk diffs (new net domains, secrets scopes) get stronger review friction. Server-side LLM jobs run at the requesting member's scope, never owner-elevated.
- **LM-16** Injection corpus (≥ 50 adversarial data-payload cases, living suite): zero unreviewed permission escalations, zero secret exfil.

## 8. Quality evaluation & cost

- **LM-17** Eval harness: ≥ 200 tasks (create/modify/debug across tracker, dashboard, form tool, automation, collab board archetypes), scored by pipeline pass + hidden tests; run on every model/prompt change; per-route scorecards gate routing expansion. Local model promotion requires ≥ 90% of cloud pass-rate on its routed class.
- **LM-18** Budgets: per-workspace monthly token budget with per-applet sub-budgets for autonomous/scheduled use; hard stop + clear UI. Prefix caching for context pack and stdlib segments.
- **LM-19** Telemetry: pipeline outcomes (pass/fail/iterations/latency) **without code or data content**; full traces are explicit per-incident opt-in; disabled entirely if the user opts out (records stay local).

## 9. Acceptance

- ≥ 85% pipeline pass on cloud route; ≥ 70% on local route's restricted class; p50 intent → installed < 60 s (cloud, simple applet).
- Offline e2e on desktop **and** web: create + use a small tracker applet with network disabled (LM Studio or in-core model).
- Context modes enforce their guarantees under test (local-only mode provably sends zero project bytes to any remote).
- Injection corpus green; provider keys never appear in context, logs, or sync payloads.

## 10. Open questions

1. Bundled credit sizing & overage pricing (M2/M4 telemetry).
2. One local model or S/M pair at v1.
3. Voice intent input timing (pipeline is input-agnostic).
