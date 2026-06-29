# Terrane App API

A Terrane app has two halves, each with its own API surface:

| Half                 | Runs in                    | Entry           | Can access                   |
| -------------------- | -------------------------- | --------------- | ---------------------------- |
| **Backend** (server) | QuickJS or Wasmtime        | `handle(input)` | Terrane resources            |
| **Client** (UI)      | the host's webview         | your page's JS  | `window.terrane.invoke(...)` |

The client talks to its own backend; the backend talks to resources. The UI has
no direct access to `ctx.resource`.

> **The `ctx.resource` reference below is generated** from the capabilities' own
> declarations, and two tests in `rust/crates/terrane-core/tests/cap/host.rs` keep it
> honest: one asserts the live runtime installs exactly the declared surface,
> the other that this doc's generated section matches. Change a resource without
> regenerating (`UPDATE_DOCS=1 cargo test`) → the build goes red.

---

## Backend (server) — `main.js` or `main.wasm`

The backend runs once per invocation in the runtime declared by
`manifest.runtime`. JS backends use embedded QuickJS through `js-runtime.run`;
WASM backends use Wasmtime through `wasm-runtime.run`. Each run is a **fresh
context** — no runtime state survives between runs, so all persistence goes
through resources.

A backend is invoked as `handle(input)` where `input` is the verb's string
argument array. From the UI: `terrane.invoke("add", "buy milk")` →
`handle(["add", "buy milk"])`. `handle` **must return a string**. JS backends
can provide `handle` one of two ways.

### Recommended: an `actions` table

Declare an `actions` object — each entry keeps its description **and** its
handler together — and the runtime synthesizes the rest: verb dispatch, the
`__actions__` self-description (so agents can **discover** the app — the MCP
`app_actions` tool), per-action `usage()`, and the unknown-verb help. One source
of truth: add an entry and it's both runnable and discoverable.

```js
var description = "A CRDT-backed todo list."; // optional one-liner

var actions = {
  add: {
    summary: "Add an item.",
    args: [{ name: "text", required: true, summary: "the item text" }],
    returns: 'e.g. "added: buy milk"',
    run: function (args, usage) { // args = everything after the verb
      var text = args.join(" ").trim();
      if (text === "") return usage(); // usage() → "usage: add <text>"
      ctx.resource.crdt.listPush("todos", text);
      return "added: " + text;
    },
  },
  list: { summary: "List items.", args: [], run: function () {/* … */} },
};
```

- `run(args, usage)` returns a **string**. `usage()` is built from the action's
  declared `args` (`<name>` for required, `[name]` optional).
- The app id and name in `__actions__` come from `manifest.json` — don't repeat
  them. The emitted shape is `terrane_api::AppActions` (`app`, `title`,
  `description`,
  `actions: [{ verb, summary, args:[{ name, required, summary }], returns }]`).

### Low-level: define `handle` yourself

If you'd rather control everything (or don't want self-description), define a
global `handle(input)` directly; the runtime leaves it untouched and never
synthesizes one.

```js
function handle(input) {
  var verb = (input || [])[0];
  // … dispatch yourself; return a string …
}
```

### `ctx` for JS backends

A global `ctx` object is injected. `ctx.resource.<namespace>` is present only
for the namespaces your `manifest.json` lists in `resources` (the sandbox: an
undeclared resource is simply absent).

### WASM backend ABI

WASM backends run in Wasmtime with no WASI and no ambient host access. A WASM
module must export:

- `memory`
- `alloc(len: i32) -> ptr: i32`
- `handle(ptr: i32, len: i32) -> i64`, or the function named by `manifest.entry`

The input bytes are a JSON array of strings. The returned `i64` packs the output
pointer in the high 32 bits and byte length in the low 32 bits; those bytes must
be UTF-8. Resource access goes through host imports in module `"terrane"`:
`resource_write(ns, method, args_json) -> i32` and
`resource_read(ns, method, args_json, out) -> i32`, where each string/buffer
argument is the same packed pointer/length pair. The runtime checks
`manifest.resources` before forwarding any resource call.

### Resources

`ctx.resource.<namespace>` exposes the platform capabilities your backend may
use — present only for the namespaces your manifest declares in `resources` (the
sandbox). App-scoped (you only ever see your own app's data) and synchronous.
**Writes** are recorded as events and reproduce deterministically on replay;
**reads** are not recorded.

The tables below are **generated** from the capabilities' declared resource
APIs. Don't hand-edit between the markers — a test regenerates them and fails if
they drift from the runtime.

For richer per-capability documentation, use the generated capability doc
surface: `terrane cap list`, `terrane cap info <namespace> --format
json|markdown|skill`, or the MCP tools `capabilities_list` and
`capability_info`. Those views come from the same capability declarations, with
internal notes hidden unless `includeInternal=true`.

<!-- generated:resource-api:start -->
#### `ctx.resource.build`

| Method | Kind |
| --- | --- |
| `ctx.resource.build.compileTs(path, source)` | read |

#### `ctx.resource.crdt`

| Method | Kind |
| --- | --- |
| `ctx.resource.crdt.mapSet(doc, key, value)` | write |
| `ctx.resource.crdt.mapGet(doc, key)` | read |
| `ctx.resource.crdt.mapAll(doc)` | read |
| `ctx.resource.crdt.mapDel(doc, key)` | write |
| `ctx.resource.crdt.listPush(doc, value)` | write |
| `ctx.resource.crdt.listInsert(doc, index, value)` | write |
| `ctx.resource.crdt.listDel(doc, index)` | write |
| `ctx.resource.crdt.listAll(doc)` | read |
| `ctx.resource.crdt.textInsert(doc, index, text)` | write |
| `ctx.resource.crdt.textDel(doc, index, len)` | write |
| `ctx.resource.crdt.textGet(doc)` | read |

#### `ctx.resource.kv`

| Method | Kind |
| --- | --- |
| `ctx.resource.kv.set(key, value)` | write |
| `ctx.resource.kv.get(key)` | read |
| `ctx.resource.kv.all()` | read |
| `ctx.resource.kv.rm(key)` | write |
| `ctx.resource.kv.scan(prefix, limit)` | read |
| `ctx.resource.kv.range(start, endExclusive, limit)` | read |
| `ctx.resource.kv.keys(prefix, limit)` | read |

#### `ctx.resource.relational_db`

| Method | Kind |
| --- | --- |
| `ctx.resource.relational_db.defineTable(table, specJson)` | write |
| `ctx.resource.relational_db.put(table, rowJson)` | write |
| `ctx.resource.relational_db.delete(table, keyJson)` | write |
| `ctx.resource.relational_db.get(table, keyJson)` | read |
| `ctx.resource.relational_db.query(table, index, queryJson)` | read |
| `ctx.resource.relational_db.tables()` | read |
| `ctx.resource.relational_db.spec(table)` | read |
<!-- generated:resource-api:end -->

For `kv`: `key` and `value` must be strings, and a missing key reads back as
`null`/`undefined` — test it with `== null` (which matches both):

```js
var kv = ctx.resource.kv;
function handle(input) {
  if (input[0] === "add") {
    kv.set("greeting", input[1]);
    return "saved";
  }
  if (input[0] === "get") {
    var v = kv.get("greeting");
    return v == null ? "(unset)" : v;
  }
  return "?";
}
```

For `crdt`: where `kv` is last-writer-wins, a `crdt` document **merges** — two
replicas that edited concurrently converge with no lost writes (the sync
foundation). Every method's first argument is a container **name** (an app can
hold many named Map/List/Text documents). Values are strings; positional
arguments (`index`, `len`) are passed as strings too and parsed as integers.
Reads come back as a string/`null` (`mapGet`, `textGet`), an object (`mapAll`),
or an array (`listAll`).

```js
var crdt = ctx.resource.crdt;
function handle(input) {
  if (input[0] === "set") {
    crdt.mapSet("prefs", input[1], input[2]);
    return "saved";
  }
  if (input[0] === "todo") {
    crdt.listPush("todo", input[1]);
    return crdt.listAll("todo").join(",");
  }
  return "?";
}
```

---

## Client (UI) — `index.html`

The UI runs in the host's webview. Its only bridge to the platform is:

```js
window.terrane.invoke(verb, ...args); // → Promise<string>
```

`invoke` calls your **own backend's** `handle([verb, ...args])` and resolves
with the string it returns (or rejects with an error string). That is the entire
client→core surface — the UI never names another app and never touches
`ctx.resource` directly.

```js
const items = JSON.parse(await window.terrane.invoke("items"));
await window.terrane.invoke("add", "buy milk");
```

A host that has no UI (CLI-only apps) simply never loads `index.html`; the same
backend works unchanged.

---

## Manifest — `manifest.json`

| Field       | Type              | Meaning                                                             |
| ----------- | ----------------- | ------------------------------------------------------------------- |
| `id`        | string            | stable app id (matches the catalog entry)                           |
| `name`      | string            | display name                                                        |
| `version`   | string            | app version                                                         |
| `runtime`   | string            | `"js"` or `"wasm"`                                                   |
| `backend`   | string            | JS backend file, e.g. `"main.js"`                                    |
| `module`    | string            | WASM module file, e.g. `"main.wasm"`                                 |
| `entry`     | string (optional) | WASM entry export; defaults to `"handle"`                            |
| `ui`        | string (optional) | UI entry file, e.g. `"index.html"`; omit for CLI-only apps          |
| `resources` | string[]          | the resource namespaces the backend may use — the sandbox allowlist |

```json
{
  "id": "todo",
  "name": "Todo",
  "version": "0.1.0",
  "runtime": "js",
  "backend": "main.js",
  "ui": "index.html",
  "resources": ["kv"]
}
```

```json
{
  "id": "counter-wasm",
  "name": "Counter WASM",
  "version": "0.1.0",
  "runtime": "wasm",
  "module": "main.wasm",
  "entry": "handle",
  "resources": ["kv"]
}
```

---

## Contract

- **Deterministic & replayable.** Every backend write is recorded as an event;
  replaying the log reproduces state without re-running your JS. Don't rely on a
  clock, randomness, or external state except through a resource.
- **Sandboxed.** You only reach the resources your manifest declares, and a
  resource only ever sees your app's own data.
- **Bounded.** A backend run has a wall-clock budget; an unbounded loop is
  interrupted.
