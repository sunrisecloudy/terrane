# Applet Authoring Guide

This guide describes the current M0a applet surface in `forge/`. It is derived from `forge/std/forge-std.d.ts`, `forge/crates/domain/src/manifest.rs`, `forge/crates/pipeline/src/lib.rs`, `forge/crates/runtime/src/lib.rs`, and `prd-merged/01-core-runtime-prd.md`.

## Entrypoint

An applet script exports one async entrypoint:

```ts
export async function main(ctx, input) {
  return { ok: true, value: { received: input } };
}
```

The runtime calls `main(ctx, input)` and expects an app result:

```ts
type AppResult =
  | { ok: true; value?: JsonValue; ui?: Node }
  | { ok: false; error: string; details?: JsonValue };
```

The current pipeline accepts TypeScript source and strips types with SWC. It does not bundle modules. Static imports are rejected for M0a; keep an applet as a single entry module.

## Host API

`ctx` is the only host capability object. Generated code must not import native/platform APIs directly.

### `ctx.storage`

Per-applet key/value storage:

```ts
await ctx.storage.set("app/counter", "1");
const value = await ctx.storage.get("app/counter");
const keys = await ctx.storage.list("app/");
await ctx.storage.delete("app/counter");
```

Storage keys must be covered by manifest storage grants. Current policy accepts exact keys and scoped trailing-prefix grants such as `app/*`; bare `*` is rejected.

### `ctx.db`

Minimal record storage:

```ts
const id = await ctx.db.insert("notes", { title: "Hello" });
const rows = await ctx.db.list("notes");
const row = await ctx.db.get("notes", id);
```

Collections must be covered by manifest `db.read` or `db.write` grants. M0a record values are JSON-compatible objects. Schema-generated types arrive later.

### `ctx.ui`

Declarative UI rendering:

```ts
ctx.ui.render({
  type: "Stack",
  testId: "root",
  direction: "v",
  gap: "sm",
  children: [
    { type: "Text", testId: "title", text: "My Notes", variant: "title" },
    { type: "Button", testId: "save", label: "Save", onTap: "save" }
  ]
});
```

The small `@forge/std` surface currently includes `Stack`, `Text`, `Button`, `TextField`, and `List`. The broader UI-2 catalog is drafted in `forge/std/ui-catalog.d.ts`; use only what the runner and renderer for your target support.

### `ctx.time` and `ctx.random`

Deterministic seams:

```ts
const now = ctx.time.now();
const random = ctx.random.next();
```

`time.now()` is a logical clock seeded by the runtime and increments by one per call. `random.next()` uses a seeded deterministic stream. These values are recorded into the run trace so replay can reproduce them.

## Manifest

Each applet has a manifest matching `forge_domain::Manifest`:

```json
{
  "entrypoint": "applet.ts",
  "min_api": "forge-api@0.1",
  "deterministic": true,
  "capabilities": {
    "storage": { "read": ["app/*"], "write": ["app/*"] },
    "db": { "read": ["notes"], "write": ["notes"] },
    "ui": true
  },
  "limits": {
    "wall_ms": 3000,
    "fuel": 10000000,
    "memory_bytes": 67108864,
    "max_host_calls": 10000,
    "storage_bytes": 10485760,
    "log_bytes": 262144
  }
}
```

`entrypoint` must be non-empty. `min_api` must start with `forge-api@`. `wall_ms`, `fuel`, `memory_bytes`, and `max_host_calls` must be greater than zero.

## Determinism

Deterministic mode is the default for M0a fixtures. A deterministic run records:

- the input;
- the applet code hash;
- every host call and response;
- the random seed and logical time start;
- the permission snapshot used by replay.

Replay re-runs the same program and compares the observed trace. A different host-call method, args, response, result, code hash, or permission snapshot is a determinism failure.

## Forbidden Constructs

The pipeline rejects escape hatches before execution, and the QuickJS runtime also poisons dynamic evaluation at the engine layer. Do not use:

- `eval`;
- `Function` or `new Function`;
- dynamic `import()`;
- static module imports in M0a;
- raw network globals such as `fetch` or `XMLHttpRequest`;
- host globals such as `process`, `require`, or mutable `globalThis` escape paths;
- prototype pollution against `__proto__` or `Object.prototype`.

## Worked Example

```ts
type Input = {
  title: string;
  body: string;
};

export async function main(ctx: any, input: Input) {
  const createdAt = ctx.time.now();
  const note = { title: input.title, body: input.body, createdAt };

  const id = await ctx.db.insert("notes", note);
  const rows = await ctx.db.list("notes");
  await ctx.storage.set("app/last-note", id);

  ctx.ui.render({
    type: "Stack",
    testId: "notes-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "heading", text: "Notes", variant: "title" },
      {
        type: "List",
        testId: "notes-list",
        items: rows.map((row: any) => ({
          type: "Text",
          testId: `note-${row.title}`,
          text: `${row.title}: ${row.body}`
        }))
      }
    ]
  });

  return { ok: true, value: { id, count: rows.length, createdAt } };
}
```

Required manifest grants for the example:

```json
{
  "entrypoint": "applet.ts",
  "deterministic": true,
  "capabilities": {
    "storage": { "read": ["app/*"], "write": ["app/*"] },
    "db": { "read": ["notes"], "write": ["notes"] },
    "ui": true
  }
}
```

## Public API docs and tests

- HTML reference: `node --no-warnings tools/build-forge-api-docs.mjs` then open `forge/docs/public-api/index.html` or `GET /docs` on `forge-server`.
- Every bundled example under `forge/examples/` has library and CLI e2e coverage (`forge_examples`, `bundled_apps_cli_e2e`).
- Contract drift gate: regenerate `artifacts/public-contract.json` after changing `forge-std.d.ts`, schemas, or examples.

## Current Gaps

- `forge-ui` renderer coverage is still growing; the broad UI catalog in `forge/std/ui-catalog.d.ts` is a spec surface ahead of full renderer parity.
- The smaller `forge/std/forge-std.d.ts` remains the safer current applet authoring target.
