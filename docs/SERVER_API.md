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
operation — **invoke**: run a verb on an app's backend (`host.run`) and return
its string. That is the same contract the CLI (`terrane host run`) and the macOS
webview (`window.terrane.invoke`) already use.

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
`tools/call` over the same host core, so an MCP client (e.g. Claude Code) can
**select an app and act on it**. The stdio host reads newline-delimited JSON-RPC
from stdin/stdout; the web host exposes the same behavior at `POST /mcp`.

Tools:

| Tool          | Input                   | Returns                                                    |
| ------------- | ----------------------- | ---------------------------------------------------------- |
| `list_apps`   | `{}`                    | the installed apps (id, name, has_ui)                      |
| `app_actions` | `{ app }`               | the app's actions (verbs + args), as the app declares them |
| `invoke`      | `{ app, verb, args[] }` | the backend's output string                                |

The intended order is **list → discover → act**: `list_apps` to find an app,
`app_actions` to learn its verbs, `invoke` to run one. `app_actions` calls the
app's reserved `__actions__` verb (see the App API), so the action list is the
app's own — not hard-coded in the host.

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
(host routes, MCP tools, capabilities, `ctx.resource`, app contract, sync) comes
from `terrane contract export` — derived from the `terrane-api`/`terrane-core`
declarations, so it can't drift (guarded by `terrane-host/tests/contract.rs`).
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
