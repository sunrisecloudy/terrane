# Terrane Host API

The contract terrane's **edge hosts** expose — the **web host** (HTTP) and the
**MCP host** (stdio JSON-RPC). It is the *open subset*: `terrane-premium`
implements a **superset** of everything here, so apps and clients written against
this surface are portable upward.

The source of truth is the [`terrane-api`](../terrane-core/crates/terrane-api)
crate (typed, OSS-side) and the exported `public-contract.json` that premium pins
(language-neutral). This document is the readable view; the crate is normative.

- Contract version: `terrane_api::CONTRACT_VERSION`
- MCP protocol version: `terrane_api::MCP_PROTOCOL_VERSION`

Both hosts run against a single `TERRANE_HOME` and ultimately call one core
operation — **invoke**: run a verb on an app's backend (`host.run`) and return
its string. That is the same contract the CLI (`terrane host run`) and the macOS
webview (`window.terrane.invoke`) already use.

---

## Web host (HTTP)

A single-threaded local server (one `TERRANE_HOME`). Loopback binds need no auth;
non-loopback binds require `Authorization: Bearer <token>`.

| Method | Path | Body | Result |
| --- | --- | --- | --- |
| `GET` | `/healthz` | — | `HealthResponse { status, version }` |
| `GET` | `/apps` | — | `AppsResponse { apps: [{ id, name, has_ui }] }` |
| `GET` | `/apps/{id}/` and `/apps/{id}/{asset}` | — | the app's UI + assets (path-traversal guarded) |
| `POST` | `/apps/{id}/invoke` | `InvokeRequest { verb, args[] }` | `InvokeResponse { output }` |

Any failure returns `ApiError { error }` with a non-2xx status.

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
    }).then(r => r.json()).then(j => j.output),
};
```

---

## MCP host (stdio JSON-RPC)

A hand-rolled stdio MCP server (`initialize` / `tools/list` / `tools/call`) over
the same invoke core, so an MCP client (e.g. Claude Code) can **select an app and
act on it**. Tools:

| Tool | Input | Returns |
| --- | --- | --- |
| `list_apps` | `{}` | the installed apps (id, name, has_ui) |
| `invoke` | `{ app, verb, args[] }` | the backend's output string |

```jsonc
// tools/call → invoke
{ "name": "invoke",
  "arguments": { "app": "todo-cli-collaborate", "verb": "add", "args": ["buy milk"] } }
// → { content: [{ "type": "text", "text": "added: buy milk" }], isError: false }
```

Every `tools/call` result carries `isError`. Tool-level failures (unknown app, a
backend error) come back as a result with `isError: true` and the message as
text — not as a JSON-RPC error — so the model sees them.

---

## Subset rule (terrane ⊆ premium)

Every route and tool above must exist in `terrane-premium` with the same request
and response shapes. Premium adds hosted concerns (accounts, orgs, billing,
encrypted sync, marketplace, signing, admin) **on top**; it never removes or
redefines anything here. The mechanical guarantee is the **conformance suite**:
the OSS hosts pass it, and premium re-runs the identical suite against the pinned
`public-contract.json` in its CI.
