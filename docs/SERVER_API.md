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

Any failure returns `ApiError { error }` with a non-2xx status.

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
| `app_actions`        | `{ app }`                                        | the app's actions (verbs + args), as the app declares them |
| `invoke`             | `{ app, verb, args[] }`                          | the backend's output string                                |
| `permission_check`   | `{ requestId }`                                  | a permission request's status and admin URL                |
| `permission_cancel`  | `{ requestId, reason? }`                         | cancel a pending request without granting access           |
| `permission_requests` | `{}`                                            | local permission requests and statuses                     |
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
| `terrane://docs/weak-models`         | `host/mcp` | locked-down model playbook                         |
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
