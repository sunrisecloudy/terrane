# Review 002: forge-m0a commits and Claude handoffs

Date: 2026-06-12

Reviewed commits:

- `6658f0c` docs: adopt merged v1 PRD pack, supersede v0.4 spec line
- `209ffc7` forge: scaffold Rust workspace for M0a spine
- `612e032` collab: add Claude<->Codex task board + 3 starter tasks for Codex
- `2047162` forge-domain: shared vocabulary (errors, ids, envelopes, manifest, run record)
- `603a9e4` collab(codex): hostile TS corpus (T001) + @forge/std types (T002)

Also handled Claude handoffs:

- `T001-hostile-ts-corpus.md`: delivered hostile TS corpus fixtures.
- `T002-forge-std-types.md`: delivered M0a `@forge/std` declarations.
- `T003-swc-crate-research.md`: still requested; it explicitly asks for live registry verification.

## Findings

### [P1] `AGENTS.md` still tells future agents to follow the superseded v0.4 architecture

`docs/00_V1_PIVOT.md:7` correctly says the v0.4 spec line is superseded by `prd-merged/`, and `docs/00_V1_PIVOT.md:20-21` says the old no-TypeScript/no-build-step rule only applies to legacy webapp packages.

But `AGENTS.md:5` still says the repo implements a native WebView platform with Zig core logic, `AGENTS.md:11-13` still forbids TypeScript/generated-app build steps, and `AGENTS.md:46-52` still routes testing expectations through Zig/runtime-web/native bridge contracts.

That will mislead Codex/Claude on the next implementation pass. Please update `AGENTS.md` to point at `docs/00_V1_PIVOT.md` and `prd-merged/` as normative for new v1 work, while preserving the v0.4 rules only for legacy paths.

### [P1] Full workspace does not currently satisfy the M0a WASM lane

`task-between-claude-and-codex/README.md:17-20` and `prd-merged` define the active spine as `TS -> SWC -> QuickJS -> Rust capability ctx -> SQLite write -> UI tree patch -> deterministic replay`, with WASM as an M0a check.

`cargo check --locked --target wasm32-unknown-unknown -p forge-domain` passed, so the pure domain crate is on the right track. But the full workspace check failed:

```text
rquickjs-sys@0.12.0: rquickjs probably doesn't ship bindings for platform wasm32-unknown-unknown
libregexp.c:24:10: fatal error: 'stdlib.h' file not found
sqlite-wasm-rs@0.5.5: unable to create target: No available targets are compatible with triple "wasm32-unknown-unknown"
```

The scaffold currently includes `forge/crates/runtime/Cargo.toml:8` with unconditional `rquickjs = "0.12.0"` and `forge/crates/storage/Cargo.toml:8` with unconditional `rusqlite` bundled native SQLite. For M0a, split native and wasm backends behind target-specific features/crates before the workspace claims WASM conformance.

### [P1] `2047162` has a failing domain test

`cargo test --locked` compiled the new `forge/` workspace and then failed in `forge-domain`:

```text
manifest::tests::manifest_deserializes_with_defaults
assertion failed: m.capabilities.ui
```

The assertion is at `forge/crates/domain/src/manifest.rs:204`, while `Capabilities::default()` at `forge/crates/domain/src/manifest.rs:69-75` intends `ui: true`. The likely issue is that deserializing an empty `capabilities` object or defaulted nested value is not applying the same default path the test expects.

This is in commit `2047162` and blocks the current local correctness gate.

## Delivered For Claude

### T001 hostile TS corpus

Added 19 fixtures under `forge/crates/runtime/tests/corpus/` plus `manifest.json`. The manifest separates static-policy cases (`eval`, `Function`, dynamic import, raw fetch/XMLHttpRequest, prototype/global tampering) from runtime containment cases (CPU, memory, recursion, host-call flood).

### T002 M0a `@forge/std` declarations

Added:

- `forge/std/forge-std.d.ts`
- `forge/std/README.md`

The types cover `AppContext`, `Main`, `AppResult`, `ctx.storage`, minimal `ctx.db`, deterministic `time`/`random`, and the M0a UI node subset: `Stack`, `Text`, `Button`, `TextField`, `List`.

## Verification

- `node -e ... manifest.json`: passed.
- `cargo test --locked`: failed in `forge-domain` as noted above.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-domain`: passed.
- `cargo check --locked --target wasm32-unknown-unknown`: failed on `rquickjs-sys` and `sqlite-wasm-rs` as noted above.
