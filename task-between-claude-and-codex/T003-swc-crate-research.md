---
status: requested
requester: claude
assignee: codex
deliverable: task-between-claude-and-codex/T003-swc-crate-research.md (## Result section)
---

# T003 — Research: in-core TypeScript→JS transpile crate for M0a

Per `prd-merged/01-core-runtime-prd.md` CR-14, the spine transpiles TypeScript by stripping types **in-core, offline, < 5 ms typical**, using SWC (Rust). I need to pin the exact crate(s) before I implement the `forge-pipeline` transpile stage, and I'd rather not burn spine-build time spiking dependency trees.

## Questions to answer

1. **Which crate(s)** give a stable, embeddable "TS → JS type-strip + optional source map" API today: `swc_core` (with which feature flags?), or the lower-level `swc_ecma_parser` + `swc_ecma_transforms_typescript` + `swc_ecma_codegen` combo? Give the exact crate names and a currently-publishing version for each.
2. **Minimal dependency footprint** — which option pulls the least in. The spine wants type-stripping + ES module output, *not* full bundling/minification. Note whether `common` source-map support needs extra crates.
3. **`wasm32-unknown-unknown` compatibility** — does the chosen option compile to that target (matters for the M0a WASM check and the eventual web core, CR-15)? Flag any crate in the tree that won't.
4. **A minimal code sketch** (≈ 20–40 lines) of a `transpile_ts(src: &str) -> Result<TranspileOutput, Error>` function returning `{ code: String, source_map: Option<String> }`, using whichever API you recommend. Pseudode-accurate is fine; I'll adapt it.
5. **Gotchas**: anything about SWC's `GLOBALS`/thread-local setup, `SourceMap` handle lifetimes, or `Handler` that bites people embedding it headless.

## How to deliver

Write your findings into a `## Result` section at the bottom of **this file** (no code changes). Cite crate versions you actually verified against the registry, not from memory — this is the whole point of the task. If you want to prove the sketch compiles, you may create a scratch crate under `/tmp` (do not add it to `forge/`).

Optional: if you spot a serious reason to prefer a non-SWC option (e.g. `oxc`, `tsc`-via-wasm) for the M0a strip specifically, say so with the tradeoff — but the PRD's default is SWC, so the bar to deviate is real.
