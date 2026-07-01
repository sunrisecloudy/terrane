# Terrane Host API

The contract terrane's **edge hosts** expose — the **web host** (HTTP plus MCP
over HTTP) and the **MCP host** (stdio JSON-RPC). It is the _open subset_:
`terrane-premium` implements a **superset** of everything here, so apps and
clients written against this surface are portable upward.

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

Tools:

| Tool                 | Input                                            | Returns                                                    |
| -------------------- | ------------------------------------------------ | ---------------------------------------------------------- |
| `workflows_list`     | `{}`                                             | guided workflow ids and first calls                        |
| `workflow_info`      | `{ name }`                                       | exact `tools/call` recipe steps for one workflow           |
| `app_recipe`         | `{ kind? }`                                      | app-building happy-path guidance                           |
| `app_scaffold`       | `{ id, name, kind?, withUi? }`                   | generated bundle files as JSON                             |
| `app_bundle_validate` | `{ path }`                                      | bundle manifest/ref validation                             |
| `app_register_inline` | `{ files, id?, name?, runtime?, dryRun? }`      | MCP-only validated app registration from inline files      |
| `app_register`       | `{ source, id?, name?, runtime?, dryRun? }`      | validated `app.add` dispatch or dry-run                    |
| `list_apps`          | `{}`                                             | the installed apps (id, name, has_ui)                      |
| `app_actions`        | `{ app }`                                        | the app's actions (verbs + args); may return `permission_required` (`isError: true`) |
| `invoke`             | `{ app, verb, args[] }`                          | the backend's output string; may return `permission_required` (`isError: true`) |
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
`app_scaffold`, `app_register_inline` with `dryRun: true`,
`app_register_inline` commit, `app_actions`, and `invoke`.
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

| Namespace | Verbs |
| --- | --- |
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

A namespace a manifest requests but that is not grantable is skipped, not
blocked.

### Weak-model recipe (copy-paste)

1. `workflows_list` → pick a workflow (e.g. `make_js_kv_app`).
2. `app_scaffold` with `{ "id": "notes-demo", "name": "Notes Demo" }` (add
   `"withUi": true` for a page).
3. `app_register_inline` with `{ "files": <structuredContent.files>, "dryRun": true }`,
   then again without `dryRun` to commit.
4. `invoke` with `{ "app": "notes-demo", "verb": "write", "args": ["hello"] }`.
5. If the result is `"isError": true` with
   `structuredContent.type == "permission_required"`, the app's namespace is
   ungranted. Do one of:
   - **Human/CLI**: run each string in `structuredContent.grantCommands`, e.g.
     `terrane auth grant user:local-owner notes-demo kv`.
   - **Admin UI**: open `structuredContent.adminUrl` and approve (trusted admin
     action).
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is `approved`.
6. Once `approved`, **retry `invoke`** with the same args → success.

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

## Subset rule (terrane ⊆ premium)

Every route and tool above must exist in `terrane-premium` with the same request
and response shapes. Premium adds hosted concerns (accounts, orgs, billing,
encrypted sync, marketplace, signing, admin) **on top**; it never removes or
redefines anything here. The mechanical guarantees:

- **Drift**: `terrane-host/tests/contract.rs` asserts the exported surface
  matches the live declarations; `tools/verify-public-contract.mjs` re-checks
  it + the contract file hashes.
- **Behaviour**: the host e2e suites are the conformance tests — e.g.
  `host/web/tests/web.rs` drives the running server and asserts it serves
  _every_ declared route. Premium re-runs the analogous black-box checks against
  its server in CI.
