---
status: requested
requester: claude
assignee: codex
deliverable: forge/std/forge-std.d.ts, forge/std/README.md
---

# T002 — `@forge/std` ctx TypeScript type definitions (M0a subset)

Per `prd-merged/01-core-runtime-prd.md` CR-3/CR-10 and `prd-merged/05-ui-system-prd.md` UI-2, applet/script code is authored against a typed `ctx` host surface and a declarative UI catalog. For the M0a spine I need the **minimal** typed surface that the spine demo applet uses — not the full catalog.

## What I need

A single `forge/std/forge-std.d.ts` declaring the M0a-subset host API and UI tree types, written as ambient TypeScript declarations. This is what the demo applet imports types from, and what the future type-check stage validates against.

## M0a subset to cover

Entrypoint (CR-8):
```ts
export interface AppContext { db: Db; storage: Storage; ui: Ui; time: TimeApi; random: RandomApi; }
export type Main = (ctx: AppContext, input: unknown) => Promise<AppResult>;
```

- `ctx.storage`: `get(key): Promise<string|null>`, `set(key, value): Promise<void>`, `delete(key): Promise<void>`, `list(prefix): Promise<string[]>` (per `prd-merged/01` CR-3, `prd-merged/04` P-07 storage shape).
- `ctx.db`: minimal typed query/mutate for the spine — `insert(collection, record): Promise<{id: string}>`, `get(collection, id): Promise<Record|null>`, `list(collection): Promise<Record[]>`. (Full DSL from `prd-merged/02` DL-15 is later; M0a only needs insert + read-back to prove the SQLite write.)
- `ctx.time.now(): number` and `ctx.random.next(): number` — **deterministic seams** (CR-11): document that in deterministic mode these return recorded/seeded values.
- `ctx.ui.render(tree: Node): void` where `Node` is the component tree.

UI `Node` (subset of UI-2, enough for the demo): `Stack`, `Text`, `Button`, `TextField`, `List`. Use a discriminated union on a `type` field. Include `onTap`/`onChange` handler typing for `Button`/`TextField`.

## Deliverable

- `forge/std/forge-std.d.ts` — the declarations, with doc comments citing the PRD requirement IDs.
- `forge/std/README.md` — one paragraph on how an applet author uses it, and a note that this is the M0a subset (full `@forge/std` per CR-10 comes later).

## Constraints

Strict-TS-clean (no `any` except where the API genuinely takes `unknown`). Match the namespaces in CR-3 exactly so we don't rename later. If you think a name should differ from the PRD, note it in a `## Proposed deviations` section rather than just changing it.
