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

### Required common verbs and items

Every JS bundle must expose these verbs. Action-table apps get scaffold defaults
from the runtime; custom `handle(input)` apps must implement and declare them in
`__actions__`.

| Verb | Interface | Required | Contract |
| --- | --- | --- | --- |
| `common.receive` | `inbox` | yes | `(kind, payloadJson)` receives inbound text/json/link/blob-style payloads and returns a string reply. |
| `common.list` | `items` | yes | `(filterJson?)` returns a JSON array of `{id,title,kind}`. `[]` is valid for apps with no natural items. |
| `common.get` | `items` | yes | `(id)` returns item JSON or typed not-found JSON: `{"ok":false,"error":{"code":"NotFound","id":"..."}}`. |

Deep links, file associations, and share-sheet deliveries enter through
`common.receive`; they cannot invoke arbitrary backend verbs. Item URIs deliver
`common.receive("link", {"item":"<itemId>"})`. File imports are stored in the
blob CAS and deliver `common.receive("blob", {"name","hash","size","mime"})`.

Items are named by `terrane://app/<appId>/item/<itemId>`, where `itemId` is
percent-encoded. The URI is a name, not a bearer token; resolving it still goes
through interop authorization and the owning app's live `common.get`.

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
when **both** conditions hold:

1. your `manifest.json` lists the namespace in `resources` (this only *requests*
   access — it does not grant it), and
2. an admin has **granted** that namespace to the executing subject for your app.

Resources are **default-deny**. Declaring a namespace in the manifest does not
auto-grant it: until the grant exists, `ctx.resource.<namespace>` is **absent**
(`undefined`). Always **feature-detect** before use — see
[Default-deny resources & the permission handshake](#default-deny-resources--the-permission-handshake).

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
use. A namespace is present only when your manifest **requests** it (in
`resources`) **and** an admin has **granted** it (default-deny — see
[Default-deny resources & the permission handshake](#default-deny-resources--the-permission-handshake)).
Resources are app-scoped (you only ever see your own app's data) and
synchronous. **Writes** are recorded as events and reproduce deterministically
on replay; **reads** are not recorded.

> **Feature-detect before use.** Because a requested namespace can be absent
> until granted, never assume `ctx.resource.<ns>` exists. Guard it:
>
> ```js
> if (!ctx.resource || !ctx.resource.kv) {
>   // Not granted yet. Return a plain string; the host turns an ungranted
>   // namespace into a permission_required result for the caller to resolve.
>   return "kv not granted yet";
> }
> ctx.resource.kv.set("greeting", "hi");
> ```

### Backend logging

JS backends always get a global `console` shim. `console.log` and
`console.info` write `info` entries, `console.warn` writes `warn`, and
`console.error` writes `error` into the local per-app ring buffer under
`$TERRANE_HOME/logs/<app>/`. Console logging is available even when the app has
not requested or been granted the `telemetry` resource, so diagnostics do not
trigger a permission prompt.

Apps that request and receive the `telemetry` grant can also call
`ctx.resource.telemetry.debug(msg, dataJson?)`, `.info(...)`, `.warn(...)`,
`.error(...)`, and `.read(level?, tail?)`. Debug/info/warn entries are local
buffer lines only; `error` also records one `telemetry.error` event so replay
rebuilds the app's error count and last error facts. The optional `dataJson`
payload stays in the local jsonl buffer; only its SHA-256 digest enters the
event log.

Thrown backend exceptions are mirrored to the local buffer with the caught error
rendering and, when telemetry is granted, one `telemetry.error` event. Logs are
not exported over the network; local owners and agents can inspect them with
`terrane logs <app> [--level warn] [--tail 200]` or the MCP `app_logs` tool.

The tables below are **generated** from the capabilities' declared resource
APIs. Don't hand-edit between the markers — a test regenerates them and fails if
they drift from the runtime.

For richer per-capability documentation, use the generated capability doc
surface: `terrane cap list`, `terrane cap info <namespace> --format
json|markdown|skill`, or the MCP tools `capabilities_list` and
`capability_info`. Those views come from the same capability declarations, with
internal notes hidden unless `includeInternal=true`.

<!-- generated:resource-api:start -->
#### `ctx.resource.blob`

| Method | Kind |
| --- | --- |
| `ctx.resource.blob.put(name, base64, mime)` | call |
| `ctx.resource.blob.get(name)` | read |
| `ctx.resource.blob.stat(name)` | read |
| `ctx.resource.blob.list(prefix)` | read |
| `ctx.resource.blob.rm(name)` | write |

#### `ctx.resource.browser`

| Method | Kind |
| --- | --- |
| `ctx.resource.browser.render(request_json)` | call |
| `ctx.resource.browser.peek(request_json)` | call |

#### `ctx.resource.build`

| Method | Kind |
| --- | --- |
| `ctx.resource.build.compileTs(path, source)` | read |

#### `ctx.resource.connection`

| Method | Kind |
| --- | --- |
| `ctx.resource.connection.list()` | read |
| `ctx.resource.connection.stat(name)` | read |

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

#### `ctx.resource.crypto`

| Method | Kind |
| --- | --- |
| `ctx.resource.crypto.newVault(masterPassword)` | read |
| `ctx.resource.crypto.unlock(masterPassword, meta)` | read |
| `ctx.resource.crypto.lock(session)` | read |
| `ctx.resource.crypto.status(session)` | read |
| `ctx.resource.crypto.seal(session, plaintext)` | read |
| `ctx.resource.crypto.open(session, blob)` | read |
| `ctx.resource.crypto.generatePassword(optionsJson)` | read |
| `ctx.resource.crypto.generatePassphrase(optionsJson)` | read |
| `ctx.resource.crypto.strength(password)` | read |
| `ctx.resource.crypto.totp(paramsJson)` | read |
| `ctx.resource.crypto.sha1Hex(text)` | read |
| `ctx.resource.crypto.randomId()` | read |

#### `ctx.resource.document`

| Method | Kind |
| --- | --- |
| `ctx.resource.document.create(id, title, body, metadataJson)` | write |
| `ctx.resource.document.patch(id, patchJson)` | write |
| `ctx.resource.document.append(id, text)` | write |
| `ctx.resource.document.delete(id)` | write |
| `ctx.resource.document.get(id)` | read |
| `ctx.resource.document.list()` | read |
| `ctx.resource.document.exportMarkdown(id)` | read |

#### `ctx.resource.history`

| Method | Kind |
| --- | --- |
| `ctx.resource.history.list(filter, before, limit)` | read |
| `ctx.resource.history.key(key, limit)` | read |
| `ctx.resource.history.at(key, seq)` | read |

#### `ctx.resource.interop`

| Method | Kind |
| --- | --- |
| `ctx.resource.interop.call(target, verb, args)` | call |
| `ctx.resource.interop.send(interface, kind, payloadJson)` | call |
| `ctx.resource.interop.pick(interface)` | call |

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
| `ctx.resource.kv.public(key)` | read |
| `ctx.resource.kv.publicScan(prefix, limit)` | read |
| `ctx.resource.kv.publicAll()` | read |
| `ctx.resource.kv.publicKeys(prefix, limit)` | read |

#### `ctx.resource.local-model`

| Method | Kind |
| --- | --- |
| `ctx.resource.local-model.ask(prompt)` | call |
| `ctx.resource.local-model.askModel(model, prompt)` | call |
| `ctx.resource.local-model.askJson(schema, prompt)` | call |
| `ctx.resource.local-model.chat(prompt)` | call |
| `ctx.resource.local-model.chatModel(model, prompt)` | call |
| `ctx.resource.local-model.embed(text)` | call |
| `ctx.resource.local-model.embedQuery(text)` | call |
| `ctx.resource.local-model.embedModel(model, text)` | call |
| `ctx.resource.local-model.pullModel(repo, file)` | call |
| `ctx.resource.local-model.resetChat()` | call |
| `ctx.resource.local-model.models()` | read |

#### `ctx.resource.media`

| Method | Kind |
| --- | --- |
| `ctx.resource.media.info(blobName)` | read |

#### `ctx.resource.native`

| Method | Kind |
| --- | --- |
| `ctx.resource.native.clipboardWriteText(requestId, text)` | write |
| `ctx.resource.native.externalOpenUrl(requestId, url)` | write |
| `ctx.resource.native.notificationShow(requestId, title, body)` | write |
| `ctx.resource.native.dialogOpenFile(requestId, optionsJson)` | write |
| `ctx.resource.native.result(requestId)` | read |
| `ctx.resource.native.pending()` | read |

#### `ctx.resource.net`

| Method | Kind |
| --- | --- |
| `ctx.resource.net.get(url)` | call |
| `ctx.resource.net.call(request_json)` | call |

#### `ctx.resource.query`

| Method | Kind |
| --- | --- |
| `ctx.resource.query.jmespath(sourceJson, expression)` | read |
| `ctx.resource.query.pipeline(sourceJson, pipelineJson)` | read |
| `ctx.resource.query.viewGet(view, key)` | read |
| `ctx.resource.query.viewScan(view, prefix, limit)` | read |
| `ctx.resource.query.viewStat(view)` | read |
| `ctx.resource.query.viewList()` | read |

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

#### `ctx.resource.scheduler`

| Method | Kind |
| --- | --- |
| `ctx.resource.scheduler.create(id, cron, timezone, action, payload)` | write |
| `ctx.resource.scheduler.list()` | read |
| `ctx.resource.scheduler.pause(id)` | write |
| `ctx.resource.scheduler.resume(id)` | write |
| `ctx.resource.scheduler.remove(id)` | write |
| `ctx.resource.scheduler.history(id, limit)` | read |

#### `ctx.resource.search`

| Method | Kind |
| --- | --- |
| `ctx.resource.search.upsert(docId, text)` | write |
| `ctx.resource.search.upsertJson(docId, docJson)` | write |
| `ctx.resource.search.remove(docId)` | write |
| `ctx.resource.search.configure(configJson)` | write |
| `ctx.resource.search.setEmbedding(docId, embeddingJson)` | write |
| `ctx.resource.search.query(text, optionsJson)` | read |
| `ctx.resource.search.bm25(text, optionsJson)` | read |
| `ctx.resource.search.vectorSearch(queryVecJson, optionsJson)` | read |
| `ctx.resource.search.status()` | read |

#### `ctx.resource.stt`

| Method | Kind |
| --- | --- |
| `ctx.resource.stt.select(sessionId, fromSeq, toSeq, sink)` | call |
| `ctx.resource.stt.stop(sessionId)` | call |
| `ctx.resource.stt.sessions()` | read |
| `ctx.resource.stt.segments(sessionId)` | read |
| `ctx.resource.stt.selections(sessionId)` | read |

#### `ctx.resource.sysinfo`

| Method | Kind |
| --- | --- |
| `ctx.resource.sysinfo.snapshot()` | read |
| `ctx.resource.sysinfo.cpu()` | read |
| `ctx.resource.sysinfo.memory()` | read |
| `ctx.resource.sysinfo.disk()` | read |
| `ctx.resource.sysinfo.network()` | read |
| `ctx.resource.sysinfo.battery()` | read |
| `ctx.resource.sysinfo.system()` | read |
| `ctx.resource.sysinfo.processes(sortBy, limit)` | read |

#### `ctx.resource.telemetry`

| Method | Kind |
| --- | --- |
| `ctx.resource.telemetry.debug(msg, dataJson?)` | call |
| `ctx.resource.telemetry.info(msg, dataJson?)` | call |
| `ctx.resource.telemetry.warn(msg, dataJson?)` | call |
| `ctx.resource.telemetry.error(msg, dataJson?)` | call |
| `ctx.resource.telemetry.read(level?, tail?)` | read |

#### `ctx.resource.time`

| Method | Kind |
| --- | --- |
| `ctx.resource.time.now()` | call |
| `ctx.resource.time.live()` | call |
| `ctx.resource.time.last()` | read |

#### `ctx.resource.tts`

| Method | Kind |
| --- | --- |
| `ctx.resource.tts.speak(text, options)` | call |
| `ctx.resource.tts.render(text, options)` | call |
| `ctx.resource.tts.voices()` | read |
| `ctx.resource.tts.renders()` | read |
<!-- generated:resource-api:end -->

For `kv`: `key` and `value` must be strings, and a missing key reads back as
`null`/`undefined` — test it with `== null` (which matches both):

For `net`: `ctx.resource.net.call(request_json)` accepts the same request JSON
as `net.request`: `method`, `url`, `headers`, optional string or `$base64`
body, `sensitiveHeaders`, `timeoutMs`, `redirect`, and `responseBody`. It is
live and unrecorded, so replay does not re-send it. To record a replay-stable
full HTTP response from trusted tooling, dispatch
`net.request <app> <request-json>` through `capability_command` after the app
has the `net` grant, or through the CLI as
`terrane net request <app> <request-json>`. Recorded request headers are
redacted before persistence for built-in sensitive names and app-declared
`sensitiveHeaders`; `{"$secret":"name"}` is reserved for future host secret
resolution and is rejected at the edge until that resolver exists.

For `tts`: `ctx.resource.tts.speak(text)` is transient and records nothing, so
replay never makes sound. `ctx.resource.tts.render(text, "--voice", voice,
"--rate", "1000")` synthesizes at the edge, writes audio bytes to the blob CAS,
and records `tts.rendered` plus `blob.stored`; read bytes later through the blob
resource. `ctx.resource.tts.renders()` returns this app's folded render
metadata as JSON.

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

For `query`: reads are deterministic over folded app state. A common pattern is
to scan JSON values from `kv`, aggregate them, and later materialize the same
pipeline from the trusted command surface for fast keyed reads:

```js
var source = JSON.stringify({ kv: { prefix: "orders/" } });
var dailyTotals = JSON.stringify([
  { $group: { _id: "$day", total: { $sum: "$total" }, count: { $count: {} } } },
  { $sort: { _id: 1 } },
]);

function handle(input) {
  if (input[0] === "previewDaily") {
    return ctx.resource.query.pipeline(source, dailyTotals);
  }
  if (input[0] === "dailyTotal") {
    return ctx.resource.query.viewGet("sales-by-day", input[1]);
  }
  return "?";
}
```

The matching materialized view definition is:

```json
{
  "source": { "kv": { "prefix": "orders/" } },
  "pipeline": [
    { "$group": { "_id": "$day", "total": { "$sum": "$total" }, "count": { "$count": {} } } },
    { "$sort": { "_id": 1 } }
  ],
  "key": "_id"
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

## Default-deny resources & the permission handshake

**The model in one paragraph.** Declaring a namespace in `manifest.json`
`resources` (`kv`, `crdt`, `relational_db`, `build`) **only requests** it.
Resources are **default-deny**: the manifest cannot grant itself anything.
Inside your backend, `ctx.resource.<ns>` stays **absent** until an admin grants
that namespace to the executing subject for your app. Until then the runtime
withholds the namespace's methods entirely — your code sees no
`ctx.resource.<ns>`. Grants are **trusted-host-only**: they are minted by the
CLI or the admin UI, never by the app or by an MCP client on its own behalf.

### What the app author must do

- **Declare** every namespace you need in `manifest.resources`. This is the
  *request*; it is necessary but not sufficient.
- **Feature-detect** `ctx.resource.<ns>` before every use (see the guard above).
  Do not assume it exists just because the manifest lists it.
- When it is absent, **degrade gracefully** — return a plain string. You do not
  raise the permission request yourself; the *host* does that automatically for
  the caller (see below). Your job is only to not crash.

```js
// Robust backend entry that never assumes kv is granted.
function handle(input) {
  var kv = ctx.resource && ctx.resource.kv;
  if (!kv) return "kv not granted yet";      // absent → degrade, don't throw
  if (input[0] === "set") { kv.set(input[1], input[2]); return "saved"; }
  if (input[0] === "get") { var v = kv.get(input[1]); return v == null ? "(unset)" : v; }
  return "?";
}
```

### What the caller (MCP client / agent) sees when a resource is ungranted

When `invoke` or `app_actions` runs against an app whose requested namespace is
not yet granted, the host returns a tool result with **`isError: true`** whose
body is a **`permission_required`** object — present **both** as
`structuredContent` and as a JSON string in `content[0].text`:

```json
{
  "content": [{ "type": "text", "text": "<the permission_required JSON as a string>" }],
  "structuredContent": {
    "type": "permission_required",
    "status": "permission_required",
    "requestId": "local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b",
    "app": "notes-demo",
    "appName": "Notes Demo",
    "org": "local",
    "subject": "user:local-owner",
    "source": "mcp_stdio",
    "missingResources": ["kv"],
    "adminUrl": "http://127.0.0.1:8780/__terrane/admin/requests/local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b",
    "grantCommands": ["terrane auth grant user:local-owner notes-demo kv"],
    "requestStatus": "pending",
    "resumeTool": "permission_check",
    "resumeTokenHash": "9f8e7d6c5b4a3210",
    "message": "permission required for app notes-demo: grant kv; open http://127.0.0.1:8780/__terrane/admin/requests/local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b"
  },
  "isError": true
}
```

Key fields (exact JSON names):

| Field | Use it to |
| --- | --- |
| `missingResources` | the namespaces that need granting (sorted list) |
| `grantCommands` | ready-to-run CLI commands, one per missing namespace |
| `adminUrl` | deep link for an admin to approve in the browser |
| `requestId` | poll status with `permission_check` |
| `resumeTool` | always `"permission_check"` — the tool to poll |
| `requestStatus` | `pending` \| `approved` \| `denied` \| `cancelled` \| `unrecorded` |

Surfacing the error also **records** the request (an `auth.permission.requested`
event), so `requestStatus` becomes `pending` and the request is immediately
listable and approvable — you do not need a separate step to create it.

> **Do not get stuck.** A `permission_required` result is **not** a dead end and
> **not** a bug in your app. It means "this namespace needs a grant". Resolve it
> via one of the three paths below, then **retry the same `invoke`** unchanged.

### How to grant (three exact paths)

**(a) CLI — verbatim from `grantCommands`.** Run each string as-is:

```sh
terrane auth grant user:local-owner notes-demo kv
```

The subject is always `user:local-owner` locally. Arg order is
`subject app namespace [verbs…]`; omit `verbs` to grant the namespace's full
verb set. There is no dedicated `auth` subcommand — any `auth.*` command
(`auth grant …`, `auth revoke …`) flows through the generic CLI path, which is a
trusted host.

**(b) Admin UI — a trusted admin action.** Open the `adminUrl` (or the admin
page at `http://127.0.0.1:8780/__terrane/admin`) and approve. Approval mints the
missing grants and marks the request `approved`. Admin routes require the
trusted header `X-Terrane-Admin: local-admin` (returning `403` otherwise), so
this is explicitly a human/admin step — a requesting agent cannot self-serve it.

**(c) MCP poll — you cannot grant yourself.** An MCP client **cannot** grant its
own access: `capability_command` refuses any `auth.*` name with
`"<name> is trusted-admin-only; use the permission request/admin approval flow"`.
So from MCP the flow is: the `permission_required` already created a pending
request → poll `permission_check` with `{ "requestId": "<requestId>" }` until
`status` is `approved` → a human/admin meanwhile runs a `grantCommands` entry or
approves at `adminUrl` → then **retry `invoke`**.

### MCP permission tools (none of these grant access)

| Tool | Input | Returns |
| --- | --- | --- |
| `permission_check` | `{ "requestId": "<id>" }` | the request's `PermissionRequestView` as `structuredContent`, or text error `"permission request not found"` |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | cancels a pending request and returns the updated view (does **not** grant; approval remains a trusted admin action) |
| `permission_requests` | `{}` | `{ "requests": [PermissionRequestView, …] }` — all local requests |

A `PermissionRequestView` has: `requestId`, `org`, `subject`, `app`, `appName`,
`operation`, `source`, `resumeTokenHash`,
`resources[]` (each `{ namespace, selectorSchemaId, resourceId, verbs[] }`),
`status`, `adminUrl`, `decidedBy`, `decisionReason`.

### Grantable namespaces & verbs

| Namespace | Verbs |
| --- | --- |
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

`terrane auth grant` with no verbs argument grants the full set for that
namespace. A namespace your manifest requests but that isn't one of these is
skipped, not blocked.

### End-to-end recipe (weak-model safe)

1. `invoke` with `{ "app": "notes-demo", "verb": "set", "args": ["greeting", "hi"] }`.
2. If the result has `"isError": true` and `structuredContent.type ==
   "permission_required"`, the app's namespace (e.g. `kv`) is ungranted. Do one of:
   - **CLI**: run each string in `structuredContent.grantCommands`
     (e.g. `terrane auth grant user:local-owner notes-demo kv`).
   - **Admin UI**: open `structuredContent.adminUrl` and approve.
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is `approved`.
3. Once granted, **retry `invoke`** with the **same** args → success.
4. Never call `capability_command` with an `auth.*` name to grant — it is
   refused as trusted-admin-only.

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

### Top bar — document name & theme

The host owns the chrome around your app (the sidebar and the top bar). Two
slices of it are shared with the app through `window.terrane`, identically on
the web and macOS hosts, so an app is portable:

```js
// Document name — the editable segment in the breadcrumb ("App / <document>").
window.terrane.getDocument();          // → current name (string)
window.terrane.setDocument("Sketch 1"); // rename it (e.g. the file you opened)
const stop = window.terrane.onDocument((name) => { /* user renamed it */ });

// Host theme — "system" | "light" | "dark".
window.terrane.getTheme();             // → current theme
window.terrane.onTheme((theme) => { /* host theme changed */ });

stop(); // every on* returns an unsubscribe function
```

`onDocument`/`onTheme` fire once with the current value as soon as the host has
synced it, then again on every change. The host persists the document name per
app. Everything is best-effort: if a host provides no top bar, `getTheme()`
returns `"system"`, `onDocument` simply never fires, and `setDocument` is a
no-op — your app keeps working.

`"system"` means "the host is not overriding — follow the OS"; resolve it with
`window.matchMedia("(prefers-color-scheme: dark)")` (WebKit already sets the
page's `color-scheme` from the OS). The web host reports the user's picker
choice (`system`/`light`/`dark`); the macOS host has no in-app override and so
always reports `"system"`.

> Security note (web host): each app frame is loaded with a fresh per-load
> nonce, and only messages carrying it drive the bridge or the breadcrumb. A
> page your app navigates its own frame to loads without the nonce, so it
> cannot invoke your backend or rename your document.

### Localization — locale, direction & translation

Localization is the third slice of host chrome shared through `window.terrane`,
identically on the web and macOS hosts. The host detects the user's language
(web: the `Accept-Language` header or the in-app language picker; macOS: the
system language), negotiates it to a supported code, and pushes the active
locale, its writing direction, and a **message bundle** down to your frame.

```js
window.terrane.getLocale();   // → active code, e.g. "en" | "es" | "zh-Hans" | "ar" (default "en")
window.terrane.getDir();      // → "ltr" | "rtl"  ("rtl" only for "ar")
window.terrane.getMessages(); // → a copy of the active bundle { "todo.add": "Añadir", … }
window.terrane.t("todo.add", { default: "Add" });          // → translate + fall back
window.terrane.t("todo.added", { id: 1, text: "milk", default: "added #{id} {text}" });

const stop = window.terrane.onLocale((code) => { /* language changed */ });
window.terrane.onMessages((messages) => { /* bundle arrived / changed → re-render */ });
```

- **`t(key, params)`** looks `key` up in the active bundle; if it is missing it
  uses `params.default` (or the key itself), then interpolates `{name}`
  placeholders from `params`. Pure string in, string out — **assign it with
  `textContent`, never `innerHTML`.**
- **Fallback contract.** With no host / no bundle, `getLocale()` is `"en"`,
  `getDir()` is `"ltr"`, `t()` returns `params.default ?? key`, and
  `onLocale`/`onMessages` still fire once. Apps keep working headless / in the
  CLI. Always pass a `default:` so the first paint is correct before the bundle
  arrives, then re-localize inside `onMessages`.
- **RTL.** Set `document.documentElement.dir = window.terrane.getDir()` and use
  CSS logical properties (`margin-inline-start`, `text-align: start`, …) so
  Arabic mirrors correctly.

#### Where translations live — the `i18n/<code>/<domain>.<key>` convention

Translations are stored once in a shared **public KV** bucket and reused across
every app and platform, under keys `i18n/<code>/<domain>.<key>` — `<domain>` is
`system` (host chrome) or your app id. Ship them as flat JSON catalogs beside
your app, one file per language, and the host seeds them:

```
apps/<id>/i18n/en.json        # { "add": "Add", "empty": "Nothing to do", … }
apps/<id>/i18n/es.json        # { "add": "Añadir", … }
```

Hosts **seed catalogs automatically on startup** (idempotent), so apps localize
out of the box; `terrane i18n import <path>` (or `terrane_i18n_import` over the C
ABI) does the same on demand. Either walks `i18n/system` and `apps/*/i18n`, keys
every entry as `i18n/<code>/<domain>.<key>`, and commits them into public KV via
a trusted-host `kv.public.import`. A backend that needs the raw strings can read
them with `ctx.resource.kv.public(key)` (a read-only, cross-app view); the UI
never needs to — the host pushes your app's merged bundle (its own domain plus
the shared `system` domain) to the frame. Apps with a `frontend` build step
rebuild their `dist/` with `terrane app build <dir>` after editing source.

Keep `en` **complete** — it is both the fallback and the key inventory. The
supported set is `en, es, zh-Hans, ar, pt-BR, fr, de, ja, id, th-TH, ko, vi`.

A minimal, copyable localize step (see `apps/todo/index.html`):

```js
function localize() {
  document.documentElement.dir = window.terrane.getDir?.() || "ltr";
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    el.textContent = window.terrane.t(el.dataset.i18n, { default: el.textContent.trim() });
  });
}
localize();
window.terrane.onMessages?.(localize); // re-localize when the bundle arrives / changes
```

> **Backend output is not auto-localized (v1).** The `window.terrane` surface
> localizes the UI. A backend's return strings, action summaries, and CLI/MCP
> output stay in the language you write them; localize the UI, which is what the
> user sees.

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
| `resources` | string[]          | the resource namespaces the backend **requests** — default-deny; each still needs an admin grant before `ctx.resource.<ns>` appears (see the [permission handshake](#default-deny-resources--the-permission-handshake)) |
| `interfaces` | string[]        | common interfaces the app advertises; `inbox` is implied and `items` is required |
| `fileTypes` | `{ext,mime}[]`    | optional file associations; installing the app records `app.link.registered {kind:"filetype", spec:"ext:mime"}` and `terrane open <file>` imports matching files through blob before calling `common.receive("blob", ref)` |

```json
{
  "id": "todo",
  "name": "Todo",
  "version": "0.1.0",
  "runtime": "js",
  "backend": "main.js",
  "ui": "index.html",
  "resources": ["kv"],
  "interfaces": ["items"],
  "fileTypes": [{ "ext": "txt", "mime": "text/plain" }]
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
- **Sandboxed & default-deny.** You only reach the resources your manifest
  declares, a resource only ever sees your app's own data, and a *declared*
  namespace is still withheld until an admin grants it (see the
  [permission handshake](#default-deny-resources--the-permission-handshake)).
- **Bounded.** A backend run has a wall-clock budget; an unbounded loop is
  interrupted.
