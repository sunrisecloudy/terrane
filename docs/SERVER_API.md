# Terrane Host API

The contract terrane's **edge hosts** expose — the **web host** (HTTP plus MCP
over HTTP) and the **MCP host** (stdio JSON-RPC). Terrane's hosts are the only
implementations of this surface. `terrane-premium` is a hosted **control
plane**, not a host: it pins and consumes this contract as
`public-contract.json` (see [Export & conformance](#export--conformance)); it
does not serve these routes or tools.

The source of truth is the [`terrane-api`](../rust/crates/terrane-api)
crate (typed, OSS-side) and the exported `public-contract.json` that premium
pins (language-neutral). This document is the readable view; the crate is
normative.

- Contract version: `terrane_api::CONTRACT_VERSION`
- MCP protocol version: `terrane_api::MCP_PROTOCOL_VERSION`

Both hosts run against a single `TERRANE_HOME` and ultimately call one core
operation — **invoke**: run a verb on an app's backend using the runtime declared
by the app manifest (`js-runtime.run` or `wasm-runtime.run`) and return its
string. That is the same contract the CLI and the macOS webview
(`window.terrane.invoke`) already use.

---

## Web host (HTTP)

A single-threaded local server (one `TERRANE_HOME`). Loopback binds need no
auth; non-loopback binds require `Authorization: Bearer <token>`.

| Method | Path                                   | Body                             | Result                                                   |
| ------ | -------------------------------------- | -------------------------------- | -------------------------------------------------------- |
| `GET`  | `/healthz`                             | —                                | `HealthResponse { status, version }`                     |
| `GET`  | `/apps`                                | —                                | `AppsResponse { apps: [{ id, name, has_ui }] }`          |
| `POST` | `/mcp`                                 | one JSON-RPC request             | MCP JSON-RPC response (`202 Accepted` for notifications) |
| `GET`  | `/apps/{id}/` and `/apps/{id}/{asset}` | —                                | the app's UI + assets (path-traversal guarded)           |
| `POST` | `/apps/{id}/invoke`                    | `InvokeRequest { verb, args[] }` | `InvokeResponse { output }`                              |
| `GET`  | `/apps/{id}/logs?level=&tail=`         | —                                | app-local backend log JSON lines                         |

Any failure returns `ApiError { error }` with a non-2xx status. An `invoke`
against a resource the app has not been granted returns a `permission_required`
payload — see [Default-deny permissions](#default-deny-permissions--the-permission-handshake).

### Admin control endpoints (`/__terrane/admin`)

The trusted admin surface. It renders the admin page, lists/deep-links
permission requests, and mints grants. **Every route here requires the trusted
admin header** `X-Terrane-Admin: local-admin`; without it the host returns
`403 "admin header required"`. Mutations are also blocked while the local admin
session is locked. These routes are how a human approves a request — the
requesting agent cannot self-serve them.

| Method | Path                                       | Body                                | Result                                            |
| ------ | ------------------------------------------ | ----------------------------------- | ------------------------------------------------- |
| `GET`  | `/__terrane/admin`                         | —                                   | the admin page                                    |
| `GET`  | `/__terrane/admin/requests/{requestId}`    | —                                   | deep link to one request (also serves admin page) |
| `POST` | `/__terrane/admin/requests/{requestId}/approve` | —                              | mints the missing grants, marks request `approved` (dispatches `auth.permission.approve`) |
| `POST` | `/__terrane/admin/grants`                  | grant spec                          | grants a namespace directly (dispatches `auth.grant`) |

Approving a request mints exactly the grants named in it. `POST /__terrane/admin/grants`
lets the admin grant a namespace without a pending request. Both are trusted
admin actions gated by the header above.

The admin surface is **host-owned** and deliberately outside the machine-pinned
contract — `public-contract.json` does not list these routes, so hosts can
evolve them without a contract version bump. The table above is representative,
not exhaustive: the web host also serves session lock/unlock, apps, grants,
agents, and audit views, request deny, and agent delegation under the same
header gate. External tooling (including `terrane-premium`) integrates through
the contract-frozen `auth.*` events and grant specs, not by scripting these
routes.

`POST /mcp` is a transport endpoint for the same shared MCP host tools exposed
over stdio. It accepts a single JSON-RPC request body and returns
`application/json`; JSON-RPC notifications (no `id`) are accepted with `202`.
`GET /mcp` is intentionally `405 Method Not Allowed` for now. MCP requests reuse
the web host's auth rule, and browser-origin requests must come from loopback or
the same host.

The web host serves a small `window.terrane.invoke(verb, ...args)` shim that
`fetch`-POSTs to `/apps/{id}/invoke`, so an app runs **unchanged** on the web
that runs in the macOS webview.

```js
// served shim, conceptually
window.terrane = {
  invoke: (verb, ...args) =>
    fetch(`/apps/${APP_ID}/invoke`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ verb, args }),
    }).then((r) => r.json()).then((j) => j.output),
};
```

---

## MCP host (shared tools; stdio and HTTP transports)

The shared MCP surface implements `initialize`, `ping`, `tools/list`, and
`tools/call`, plus read-only `resources/*` and guided `prompts/*`, over the same
host core. An MCP client can **select an app and act on it** while also reading
the host-level MCP manual and capability-owned docs. The stdio host reads
newline-delimited JSON-RPC from stdin/stdout; the web host exposes the same
behavior at `POST /mcp`.

Tools (`terrane-api::mcp_tools()`, 27 total):

| Tool                 | Input                                            | Returns                                                    |
| -------------------- | ------------------------------------------------ | ---------------------------------------------------------- |
| `workflows_list`     | `{}`                                             | guided workflow ids and first calls                        |
| `workflow_info`      | `{ name }`                                       | exact `tools/call` recipe steps for one workflow           |
| `app_recipe`         | `{ kind? }`                                      | app-building happy-path guidance                           |
| `app_build_start`    | `{ id, name, kind?, withUi? }`                   | creates a server-side draft app bundle                     |
| `app_build_put_file` | `{ draftId, path?, content?, files? }`           | writes one or more complete draft files                    |
| `app_build_get`      | `{ draftId, path?, includeContent? }`            | draft file summaries or one file's content                 |
| `app_build_list`     | `{}`                                             | draft ids/status for recovery after stalls                 |
| `app_build_validate` | `{ draftId }`                                    | validates a draft and returns a validation token            |
| `app_build_commit`   | `{ draftId, validationToken? }`                  | commits a validated draft through `app.add`                |
| `app_build_discard`  | `{ draftId }`                                    | discards a server-side draft                               |
| `app_scaffold`       | `{ id, name, kind?, withUi? }`                   | generated bundle files as JSON                             |
| `app_bundle_validate` | `{ path }`                                      | bundle manifest/ref validation                             |
| `app_register_inline` | `{ files, id?, name?, runtime?, dryRun? }`      | MCP-only validated app registration from inline files      |
| `app_register`       | `{ source, id?, name?, runtime?, dryRun? }`      | validated `app.add` dispatch or dry-run                    |
| `app_upgrade`        | `{ app, source?, toVersion?, fromDraft? }`       | trusted local app upgrade through `app.upgrade`            |
| `app_install`        | `{ source }`                                     | trusted signed `.terrane` archive install                  |
| `list_apps`          | `{}`                                             | the installed apps (id, name, has_ui)                      |
| `app_actions`        | `{ app }`                                        | the app's actions (verbs + args); may return `permission_required` (`isError: true`) |
| `invoke`             | `{ app, verb, args[] }`                          | the backend's output string; may return `permission_required` (`isError: true`) |
| `app_logs`           | `{ app, level?, tail? }`                         | app-local backend log JSON lines                           |
| `permission_check`   | `{ requestId }`                                  | the `PermissionRequestView` for that request (`structuredContent`), or text error `"permission request not found"` |
| `permission_cancel`  | `{ requestId, reason? }`                         | cancels a pending request (dispatches `auth.permission.cancel`); returns the updated view. Does **not** grant access |
| `permission_requests` | `{}`                                            | `{ requests: [PermissionRequestView, …] }` — all local requests |
| `capabilities_list`  | `{ includeInternal? }`                           | capability namespaces, statuses, and short summaries       |
| `capability_info`    | `{ namespace, format?, includeInternal? }`       | one capability's docs as `json`, `markdown`, or `skill`    |
| `capability_query`   | `{ capability, query, args[] }`                  | a read-only capability query result as JSON text           |
| `capability_command` | `{ name, args[], dryRun?, help? }`               | ordered command help, record count/output, or dry-run validation |

The intended order is **list → discover → act**: `list_apps` to find an app,
`app_actions` to learn its verbs, `invoke` to run one. `app_actions` calls the
app's reserved `__actions__` verb (see the App API), so the action list is the
app's own — not hard-coded in the host.

For weaker or blank-context models, the intended order starts one step earlier:
`workflows_list` → choose a workflow from `chooseByOutcome` → `workflow_info` →
exact tool calls. The `make_js_kv_app` workflow routes agents through
`app_build_start`, one or more `app_build_put_file` calls,
`app_build_validate`, `app_build_commit`, `app_actions`, and `invoke`.
The harder `make_js_multicap_app_no_filesystem` workflow uses
`app_scaffold` kind `js_multicap_audit` and proves five capability surfaces:
`app`, `kv`, `crdt`, `relational_db`, and `replica`. Evaluation-style runs
must invoke `summary` as its own read both before and after `clearKv`. `seed`
and `clearKv` return JSON summaries too, but they do not replace the explicit
pre-clear and post-clear `summary` calls that prove readable state around each
mutation.
Clients that already have a bundle directory can use `app_bundle_validate` and
`app_register` instead.

Resources:

| Resource/template                    | Owner      | Contents                                           |
| ------------------------------------ | ---------- | -------------------------------------------------- |
| `terrane://docs/index`               | `host/mcp` | overall MCP guide                                  |
| `terrane://docs/clients`             | `host/mcp` | client configuration and raw JSON-RPC examples     |
| `terrane://docs/app-building`        | `host/mcp` | app-building workflows                             |
| `terrane://docs/capability-operations` | `host/mcp` | direct capability query/command guidance         |
| `terrane://docs/security`            | `host/mcp` | local MCP security and permission notes            |
| `terrane://docs/agent-playbook`         | `host/mcp` | agent playbook (no-source, permission handshake)                         |
| `terrane://capabilities/{namespace}` | cap crate  | capability-owned docs rendered from `src/doc.rs`   |
| `terrane://workflows/{name}`         | `host/mcp` | executable workflow JSON                           |
| `terrane://apps/{id}/actions`        | app        | app-declared action JSON                           |

Prompts:

| Prompt                    | Purpose                                                 |
| ------------------------- | ------------------------------------------------------- |
| `make_js_kv_app`          | build, register, inspect, and invoke a JS kv app        |
| `register_app_bundle`     | validate and register an existing bundle directory      |
| `inspect_app_actions`     | list apps and inspect an app's verbs                    |
| `safe_capability_command` | run command help and dry-run before low-level dispatch  |

JSON-returning tools include `structuredContent` alongside the compatibility
text block, so clients do not need to parse JSON out of MCP text content.
Tool-shape errors include a concrete `tools/call` example where possible.

`capability_query` and `capability_command` are advanced capability-operation
tools. `capability_query` is read-only and cannot append records. `capability_command`
runs through the same core dispatcher as CLI commands. Use `help: true` with a
dotted command name, for example `{ "name": "app.add", "help": true }`, to fetch
ordered argv params, returns, errors, emitted events, effects, and examples
without dispatching. Use `dryRun: true` to validate simple commit commands
without committing; effect/runtime commands reject dry-run.

Capability documentation follows the same generated-source rule:
`capabilities_list` discovers namespaces and `capability_info` returns the
canonical `CapabilityDoc` render. `includeInternal` defaults to `false`, so
implementation notes such as reserved backing-store layouts are hidden from
agents and app authors unless explicitly requested.

The ownership boundary is intentional: `host/mcp/docs` owns the overall MCP
manual, while each `terrane-cap-*` crate owns its capability document.

```jsonc
// tools/call → invoke
{
  "name": "invoke",
  "arguments": {
    "app": "todo-cli-collaborate",
    "verb": "add",
    "args": ["buy milk"]
  }
}
// → { content: [{ "type": "text", "text": "added: buy milk" }], isError: false }
```

Every `tools/call` result carries `isError`. Tool-level failures (unknown app, a
backend error) come back as a result with `isError: true` and the message as
text — not as a JSON-RPC error — so the model sees them.

---

## Sync Contract

The exported sync slice is intentionally narrow and source-derived:

| Field | Value |
| --- | --- |
| `wire_event` | `crdt.update` |
| `syncable_event_kinds` | `crdt.update` |
| `transports` | `file: terrane sync <app> --from <home>`; `tcp: terrane serve / terrane sync <app> --peer <addr>` |

The file path exchanges log/CRDT state from another local home. The TCP path is
the sync-v2 peer transport: one side runs `terrane serve [--addr <addr>]`, the
other runs `terrane sync <app> --peer <addr>`, and each connection exchanges
version vectors plus only the CRDT deltas the other side lacks. Host edges may
serve additional sync helper routes, but only the transports above are part of
the exported `SyncInfo` contract until `terrane-api` declares more.

---

## Default-deny permissions & the permission handshake

Terrane resources are **default-deny**. Declaring a resource in `manifest.json`
(`kv`, `crdt`, `relational_db`, `build`) **does not auto-grant it** — the
manifest only _requests_ a namespace. Inside the app backend, `ctx.resource.<ns>`
is **absent** until an admin grants that namespace to the executing subject for
that app. Grants are **trusted-host-only**: any `auth.*` command is rejected
unless the request carries trusted-host authority, so an MCP client **cannot
grant itself** access.

The local subject throughout is **`user:local-owner`**.

### What a client sees when a resource is ungranted

Only `invoke` and `app_actions` produce this. When either runs against an app
with an ungranted requested namespace, the host records a permission request
(as a side effect) and returns a `permission_required` object as an **error**
result — `isError: true`, with the object appearing **both** as
`structuredContent` and as a JSON-string copy in `content[0].text`:

```jsonc
{
  "content": [{ "type": "text", "text": "<the permission_required JSON as a string>" }],
  "structuredContent": { /* the permission_required object below */ },
  "isError": true
}
```

The `permission_required` object (exact JSON keys):

| Key | Value |
| --- | --- |
| `type` | `"permission_required"` |
| `status` | `"permission_required"` |
| `requestId` | deterministic id, e.g. `local-<app>-<subject>-<ns>-<hash>` |
| `app` | app id |
| `appName` | app display name |
| `org` | `local` |
| `subject` | `user:local-owner` |
| `source` | caller source: `"cli"`, `"host"`, `"mcp_stdio"`, or `"mcp_http"` |
| `missingResources` | sorted list of ungranted namespaces |
| `adminUrl` | `http://127.0.0.1:8780/__terrane/admin/requests/<requestId>` |
| `grantCommands` | one CLI command per missing namespace (see below) |
| `requestStatus` | `pending` \| `approved` \| `denied` \| `cancelled` \| `unrecorded` |
| `resumeTool` | `"permission_check"` |
| `resumeTokenHash` | 16-hex hash of the request id |
| `message` | `permission required for app <app>: grant <ns1, ns2>; open <adminUrl>` |

Because surfacing the request records it (`auth.permission.requested`),
`requestStatus` is `pending` and the request is immediately listable via
`permission_requests` and approvable at `adminUrl`.

Example:

```json
{
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
}
```

### How to grant (three ways)

Only a trusted host can grant. Pick one:

**(a) CLI — verbatim from `grantCommands`.** One command per missing namespace,
arg order `subject app namespace [verbs…]`:

```sh
terrane auth grant user:local-owner <app> <namespace>
# e.g.
terrane auth grant user:local-owner notes-demo kv
```

Omitting `verbs` grants the namespace's full verb set; an explicit `verbs` arg
is validated against the allowed set. There is no dedicated `auth` subcommand —
any `auth.*` command name (`auth grant …`, `auth revoke …`,
`auth agent.register …`) flows through the generic `<ns> <verb> [args…]` CLI
path, and because the CLI is a trusted host it passes the admission gate.

**(b) Admin UI — open `adminUrl` and approve.** A trusted admin action:

```
http://127.0.0.1:8780/__terrane/admin                          # the admin page
http://127.0.0.1:8780/__terrane/admin/requests/<requestId>     # deep link to the request
```

Approving posts to `POST /__terrane/admin/requests/<requestId>/approve`, which
mints the missing grants and marks the request `approved`. All
`/__terrane/admin` routes require `X-Terrane-Admin: local-admin`. The
requesting agent cannot self-serve this.

**(c) MCP client — request, poll, retry.** An MCP client **cannot** grant
itself: `capability_command` refuses any `auth.*` name with
`"<name> is trusted-admin-only; use the permission request/admin approval flow"`.
So the MCP flow is:

1. The recorded `permission_required` already created a `pending` request.
2. Poll `permission_check` with `{ "requestId": <id> }` until `status` is
   `approved`.
3. A human/admin approves at `adminUrl` (or runs the `grantCommands`).
4. Retry `invoke` with the same args → success.

### The permission MCP tools

None of these grants access — approval is always a trusted admin action.

| Tool | Input | Returns |
| --- | --- | --- |
| `permission_check` | `{ "requestId": "<id>" }` | the `PermissionRequestView` (`structuredContent`), or text error `"permission request not found"` |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | cancels a pending request (`auth.permission.cancel`), returns the updated view. Does **not** grant |
| `permission_requests` | `{}` | `{ "requests": [PermissionRequestView, …] }` — all local requests |

`PermissionRequestView` fields: `requestId`, `org`, `subject`, `app`, `appName`,
`operation`, `source`, `resumeTokenHash`, `resources[]` (each
`{ namespace, selectorSchemaId, resourceId, verbs[] }`), `status`, `adminUrl`,
`decidedBy`, `decisionReason`.

Status values an agent must handle: `pending` | `approved` | `denied` |
`cancelled` (the `unrecorded` value appears only on
`permission_required.requestStatus`, when a request was not yet recorded).

### Grantable namespaces & verbs

Derived from every registered capability's grant spec:

| Namespace | Selector schema | Verbs |
| --- | --- | --- |
| `applescript` | `namespace.v1` | `call`, `read` |
| `automation` | `namespace.v1` | `read`, `write` |
| `blob` | `namespace.v1` | `read`, `write`, `call` |
| `browser` | `namespace.v1` | `call` |
| `build` | `namespace.v1` | `read` |
| `common` | `namespace.v1` | `call`, `read` |
| `connection` | `namespace.v1` | `read`, `call` |
| `crdt` | `namespace.v1` | `read`, `write` |
| `crypto` | `namespace.v1` | `read` |
| `document` | `namespace.v1` | `read`, `write` |
| `geo` | `namespace.v1` | `call`, `read` |
| `history` | `namespace.v1` | `read` |
| `interop` | `namespace.v1` | `call` |
| `job` | `namespace.v1` | `read`, `write`, `call` |
| `kv` | `namespace.v1` | `read`, `write` |
| `local-model` | `namespace.v1` | `call`, `read` |
| `mcp` | `namespace.v1` | `call` |
| `media` | `namespace.v1` | `call`, `read` |
| `model` | `namespace.v1` | `call` |
| `native` | `namespace.v1` | `read`, `write` |
| `native` | `native.operation.v1` | `write` |
| `net` | `namespace.v1` | `call` |
| `presence` | `namespace.v1` | `call`, `read`, `publish`, `subscribe` |
| `push` | `namespace.v1` | `call`, `read`, `subscribe` |
| `query` | `namespace.v1` | `read`, `write` |
| `relational_db` | `namespace.v1` | `read`, `write` |
| `scheduler` | `namespace.v1` | `read`, `write` |
| `search` | `namespace.v1` | `read`, `write` |
| `stream` | `namespace.v1` | `read` |
| `stt` | `namespace.v1` | `call`, `read` |
| `sysinfo` | `namespace.v1` | `read` |
| `telemetry` | `namespace.v1` | `call`, `read` |
| `time` | `namespace.v1` | `call`, `read` |
| `tts` | `namespace.v1` | `call`, `read` |
| `webhook` | `namespace.v1` | `read` |

A namespace a manifest requests but that is not grantable is skipped, not
blocked.

### Weak-model recipe (copy-paste)

1. `workflows_list` → pick a workflow (e.g. `make_js_kv_app`).
2. `app_build_start` with `{ "id": "notes-demo", "name": "Notes Demo", "withUi": true }`
   → a server-side draft seeded from a working app shell; note the returned
   `draftId`.
3. `app_build_put_file` with `{ "draftId": ..., "path": "main.js", "content": ... }`
   for each file you change (the shell files are already in the draft).
4. `app_build_validate` with `{ "draftId": ... }` → note the returned
   `validationToken`.
5. `app_build_commit` with `{ "draftId": ..., "validationToken": ... }` to
   install the app.
6. `invoke` with `{ "app": "notes-demo", "verb": "write", "args": ["hello"] }`.
7. If the result is `"isError": true` with
   `structuredContent.type == "permission_required"`, the app's namespace is
   ungranted. Do one of:
   - **Human/CLI**: run each string in `structuredContent.grantCommands`, e.g.
     `terrane auth grant user:local-owner notes-demo kv`.
   - **Admin UI**: open `structuredContent.adminUrl` and approve (trusted admin
     action).
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is `approved`.
8. Once `approved`, **retry `invoke`** with the same args → success.

(`app_scaffold` + `app_register_inline` remain available as compatibility
tools; the staged draft flow above is primary.)

Do **not** try `capability_command` with an `auth.*` name — it is refused as
trusted-admin-only.

---

## Export & conformance

The full machine-readable contract is generated, not hand-written:

```sh
terrane contract export                         # the surface (JSON), from the Rust declarations
node tools/export-public-contract.mjs --out public-contract.json   # + provenance, license, conformance, file hashes
node tools/verify-public-contract.mjs --contract public-contract.json   # self-check (surface + file hashes)
```

`public-contract.json` is the artifact `terrane-premium` pins. Its `surface`
(host routes, MCP tools, capabilities, capability docs, `ctx.resource`, app
contract, sync) comes from `terrane contract export` — derived from the
`terrane-api`/`terrane-core` declarations, so it can't drift (guarded by
`terrane-host/tests/contract.rs`).
Its `conformance.commands` are what a consumer runs to prove an implementation.

## Consumption rule (hosts implement, premium pins)

Terrane's hosts (CLI, web, MCP, macOS) are the only implementations of this
surface. `terrane-premium` is a hosted control plane — accounts, orgs, billing,
encrypted sync, marketplace, signing, admin governance — that **consumes** the
contract rather than implementing it; it never redefines app-visible semantics.
Its stable integration points are:

- the pinned `public-contract.json` (hash-verified on every premium release);
- the `auth.*` event vocabulary and the per-namespace grant specs;
- the capability docs and the `crdt.update` sync wire.

The mechanical guarantees:

- **Drift**: `terrane-host/tests/contract.rs` asserts the exported surface
  matches the live declarations; `tools/verify-public-contract.mjs` re-checks
  it + the contract file hashes.
- **Behaviour**: the host e2e suites are the conformance tests — e.g.
  `host/web/tests/web.rs` drives the running server and asserts it serves
  _every_ declared route. Premium re-runs the contract's
  `conformance.commands` against its pinned checkout in CI.
