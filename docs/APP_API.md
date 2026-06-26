# Terrane App API

A Terrane app has two JavaScript halves, each with its own API surface:

| Half | Runs in | Entry | Can access |
| --- | --- | --- | --- |
| **Backend** (server) | the host's QuickJS runtime | `handle(input)` | `ctx.resource.*` |
| **Client** (UI) | the host's webview | your page's JS | `window.terrane.invoke(...)` |

The client talks to its own backend; the backend talks to resources. The UI has
no direct access to `ctx.resource`.

> **The `ctx.resource` reference below is generated** from the capabilities'
> own declarations, and two tests in `terrane-core/tests/cap/host.rs` keep it
> honest: one asserts the live runtime installs exactly the declared surface, the
> other that this doc's generated section matches. Change a resource without
> regenerating (`UPDATE_DOCS=1 cargo test`) → the build goes red.

---

## Backend (server) — `main.js`

The backend runs in an embedded QuickJS once per `host.run` invocation. Each run
is a **fresh context** — no JavaScript state survives between runs, so all
persistence goes through resources. Calls are **synchronous** (no Promises).

### Entry point

```js
function handle(input) {
  // input is the argument array, e.g. ["add", "buy milk"]
  // return a string (printed by a CLI host, or resolved to the UI)
}
```

- `input` is an array of string arguments. From the CLI:
  `terrane host run <app> add "buy milk"` → `handle(["add", "buy milk"])`. From
  the UI: `terrane.invoke("add", "buy milk")` → `handle(["add", "buy milk"])`.
- `handle` **must return a string** (returning anything else is an error).

### `ctx`

A global `ctx` object is injected. `ctx.resource.<namespace>` is present only
for the namespaces your `manifest.json` lists in `resources` (the sandbox: an
undeclared resource is simply absent).

### Resources

`ctx.resource.<namespace>` exposes the platform capabilities your backend may
use — present only for the namespaces your manifest declares in `resources` (the
sandbox). App-scoped (you only ever see your own app's data) and synchronous.
**Writes** are recorded as events and reproduce deterministically on replay;
**reads** are not recorded.

The tables below are **generated** from the capabilities' declared resource APIs.
Don't hand-edit between the markers — a test regenerates them and fails if they
drift from the runtime.

<!-- generated:resource-api:start -->
#### `ctx.resource.kv`

| Method | Kind |
| --- | --- |
| `ctx.resource.kv.set(key, value)` | write |
| `ctx.resource.kv.get(key)` | read |
| `ctx.resource.kv.all()` | read |
| `ctx.resource.kv.rm(key)` | write |
<!-- generated:resource-api:end -->

For `kv`: `key` and `value` must be strings, and a missing key reads back as
`null`/`undefined` — test it with `== null` (which matches both):

```js
var kv = ctx.resource.kv;
function handle(input) {
  if (input[0] === "add") { kv.set("greeting", input[1]); return "saved"; }
  if (input[0] === "get") { var v = kv.get("greeting"); return v == null ? "(unset)" : v; }
  return "?";
}
```

---

## Client (UI) — `index.html`

The UI runs in the host's webview. Its only bridge to the platform is:

```js
window.terrane.invoke(verb, ...args) // → Promise<string>
```

`invoke` calls your **own backend's** `handle([verb, ...args])` and resolves with
the string it returns (or rejects with an error string). That is the entire
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

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | stable app id (matches the catalog entry) |
| `name` | string | display name |
| `version` | string | app version |
| `backend` | string | backend JS file, e.g. `"main.js"` |
| `ui` | string (optional) | UI entry file, e.g. `"index.html"`; omit for CLI-only apps |
| `resources` | string[] | the resource namespaces the backend may use — the sandbox allowlist |

```json
{
  "id": "todo",
  "name": "Todo",
  "version": "0.1.0",
  "backend": "main.js",
  "ui": "index.html",
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
