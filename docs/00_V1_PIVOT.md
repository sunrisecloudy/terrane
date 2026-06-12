# v1 Pivot: Supersession Notice

**Date:** 2026-06-12 · **Status:** Normative

## Decision

The Terrane v0.4 specification line (`docs/00_PRD.md` and the docs it governs) is **superseded as product direction** by the merged v1 PRD pack at [`prd-merged/`](../prd-merged/README.md).

| | v0.4 (legacy/prototype) | v1 (normative) |
|---|---|---|
| Core | Zig state machine + Zig server | Single Rust core (`forge-core`) |
| Generated apps | Build-free HTML/CSS/vanilla JS packages | TypeScript applets/scripts, SWC-transpiled |
| Execution | Sandboxed iframe/WebView per host | QuickJS-WASM / QuickJS-native / JavaScriptCore behind a `JsEngine` trait |
| App UI | Apps render their own HTML | Declarative component tree, native renderers |
| Data | Fixed SQLite schema | Loro CRDT over SQLite KV/oplog + dynamic relational schema |
| Hosts | Five hand-written native hosts vs. reference-host oracle | Thin shells over generated bindings + conformance kits |

## Status of the v0.4 line

- `docs/00_PRD.md` and v0.4 specs remain in the repo as **reference for the prototype implementation** (zig-core, server, runtime-web, native hosts, reference-host). They are no longer the target of new work.
- The v0.4 rule "no TypeScript / no build step for generated apps" **does not apply** to the v1 plan; v1 makes TypeScript authoring (with an offline in-core transpile) the core product. The v0.4 rule remains true only of the legacy webapp packages.
- Salvage targets from v0.4 (testing discipline, fixtures, signing approach, security model, example apps as scenarios) are noted in `prd-merged/DECISIONS.md` context; the implementations are not carried forward.

## Where the normative rules now live

`prd-merged/00-master-prd.md` and its sub-PRDs (01–09), with conflict resolutions recorded in `prd-merged/DECISIONS.md`.
