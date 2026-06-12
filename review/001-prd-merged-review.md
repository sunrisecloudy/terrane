# Review 001: prd-merged

Date: 2026-06-12

## Summary

`prd-merged/` is the right direction. It finally matches the intended product: TypeScript authoring, Rust core, QuickJS/Wasm execution, local-first SQLite/Loro, capability sandbox, and a headless template-first implementation path.

I would promote it, but not as-is. A few sharp edges need fixing before it becomes the new source of truth.

## Findings

### [P1] The merged PRD contradicts the current normative repo rules

`AGENTS.md` says `docs/00_PRD.md` is normative and forbids TypeScript/generated-app build steps, while `prd-merged/00-master-prd.md` makes TypeScript applets the core product.

If we accept this pivot, we need an explicit supersession doc:

> v0.4 WebView/Zig/vanilla JS is legacy/prototype; v1 Rust/TS/QuickJS is now normative.

### [P1] M0 is too large to be an 8-10 week milestone

`prd-merged/09-roadmap-quality-gates-prd.md` asks for Rust workspace, SQLite, Loro, schema registry, QuickJS + JSC, RBAC, replay, SWC, type-check, policy scan, repair loop, LM Studio, UI tree, sync, CLI, renderer zero, and WASM CI.

That is a whole company milestone. Split it into:

- `M0a`: core executable spine.
- `M0b`: conformance/template.

### [P1] The TS on QuickJS on Wasm on Rust requirement should be the central acceptance test

`DECISIONS.md` makes JSC co-equal from day one. That may be right for Apple later, but the non-negotiable spine should be:

> Rust core compiled to Wasm, SWC transpiles TypeScript, QuickJS-WASM executes it, capability `ctx` calls are mediated by Rust, all offline.

Put that as the first M0 demo.

### [P2] SQLite schema uses `JSONB`, which is not SQLite-native

`prd-merged/02-data-layer-prd.md` defines `records(... data JSONB ...)`.

For a single-file SQLite workspace, use `TEXT` JSON with JSON1, or `BLOB` with explicit encoding. Keep Postgres `JSONB` as a server-scale projection later.

### [P2] Web/mobile type-check and binary-size budgets conflict

`prd-merged/01-core-runtime-prd.md` requires full offline type-check everywhere, but also budgets a 6 MB gzipped WASM core.

The TypeScript checker must be a lazy optional worker artifact, outside the initial core budget.

### [P2] Encryption/server-readable mode needs a sharper product rule

`prd-merged/03-sync-server-prd.md` depends on server-readable data for search/share/server LLM jobs, while `prd-merged/02-data-layer-prd.md` supports project-level encryption keys.

Define explicit modes:

- Server-readable workspace: enables cloud search, share links, and server LLM jobs.
- Encrypted workspace: disables or degrades those features.

## Recommendation

Use `prd-merged/` as the new v1 plan, but first make a short `docs/00_V1_PIVOT.md` or replace `docs/00_PRD.md` with a clear supersession header.

Then tighten M0 around one proof:

```text
TS source
  -> SWC
  -> QuickJS-WASM
  -> Rust capability ctx
  -> SQLite write
  -> UI tree patch
  -> deterministic replay
```

That is the jewel. Everything else should orbit it.
