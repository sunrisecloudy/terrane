# @forge/std M0a Types

`forge-std.d.ts` declares the minimal `@forge/std` TypeScript surface for the M0a executable spine. Applet and script authors import `AppContext`, `Main`, `AppResult`, and `Node` from `@forge/std`, then interact only through the provided `ctx` object: storage, minimal DB insert/read/list, deterministic time/random seams, and declarative UI rendering.

This is intentionally smaller than the full `@forge/std` described by `prd-merged/01-core-runtime-prd.md` CR-10. The full typed query DSL, capability helpers, richer UI catalog, and schema-generated record types come later.

## Public API reference

- Markdown index: `forge/docs/public-api-reference.md`
- HTML page (generated): `node --no-warnings tools/build-forge-api-docs.mjs` → `forge/docs/public-api/index.html`
- Served live: `GET /docs` on `forge-server` when the console is enabled

## Proposed deviations

- The task sketch used `Record|null` for database reads. This file uses `DbRecord | null` to avoid shadowing TypeScript's built-in `Record<K, V>` utility type.
- M0a UI event handlers are represented as serializable `ActionRef` strings instead of function callbacks. Rendered trees must cross the Rust core/renderer boundary, so functions are not serializable; ergonomic helpers can wrap this later.
