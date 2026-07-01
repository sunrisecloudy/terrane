# Terrane MCP Security

Terrane MCP operates one local `TERRANE_HOME`. Treat it as a local admin surface
unless the host is explicitly wrapped by auth and policy.

## Default-Deny Resource Model (read this first)

Declaring a resource in an app's `manifest.json` (`kv`, `crdt`, `relational_db`,
`build`) **no longer auto-grants it**. Resources are **default-deny**:

- A manifest only *requests* a namespace. It does not receive it.
- Inside the app backend, `ctx.resource.<ns>` is **absent** until an admin grants
  that namespace to the executing subject for that app.
- The runtime gate withholds the resource methods entirely when there is no
  grant — the app sees no `ctx.resource.<ns>` at all (not an empty stub).

This is why apps do not auto-receive resources: granting a namespace is a
**trusted admin decision**, made per subject/app/namespace, never something the
requesting app or an MCP client can self-serve. The local subject throughout is
`user:local-owner`.

Grants live in a strict grant table keyed by org / subject / app / resource; with
no matching row the namespace stays denied. `auth.*` commands (grant, revoke,
approve, agent register/clamp) are **trusted-host-only** — they are accepted only
from a trusted host (the CLI or the web admin UI with the admin header), and are
rejected on any untrusted path.

## What you see when a resource is denied

The `invoke` / `app_actions` app-runtime tools and grant-gated direct
`capability_command` resource commands can hit an ungranted namespace. When they
do, the host returns an **error** tool result carrying a `permission_required`
object. The tool result looks like this:

```json
{
  "content": [{ "type": "text", "text": "<the permission_required JSON as a string>" }],
  "structuredContent": { "...the permission_required object..." },
  "isError": true
}
```

So: `isError: true`, and the same object appears **both** in `structuredContent`
and as a JSON string in `content[0].text`. Inspect `structuredContent` — that is
where the actionable next steps live.

### The `permission_required` object

Exact JSON keys (do not rename):

| Key | Meaning |
|---|---|
| `type` | always `"permission_required"` |
| `status` | always `"permission_required"` |
| `requestId` | deterministic id, e.g. `local-<app>-<subject>-<ns>-<hash>` |
| `app` | app id |
| `appName` | app display name |
| `org` | `local` |
| `subject` | `user:local-owner` |
| `operation` | app/runtime verb or direct operation, e.g. `capability_command:kv.set` |
| `source` | caller source (`cli`, `host`, `mcp_stdio`, `mcp_http`) |
| `missingResources` | sorted list of ungranted namespaces |
| `adminUrl` | admin deep link to approve this request |
| `grantCommands` | one copy-paste CLI command per missing namespace |
| `requestStatus` | `pending` \| `approved` \| `denied` \| `cancelled` \| `preview` \| `unrecorded` |
| `resumeTool` | `"permission_check"` for recorded requests; empty for dry-run previews |
| `resumeTokenHash` | 16-hex hash of the request id |
| `message` | human message: `permission required for app <app>: grant <ns…>; open <adminUrl>` |

Surfacing a real denial also **records** the request (an
`auth.permission.requested` event), so `requestStatus` becomes `pending` and the
request is immediately listable, checkable, and approvable. You do not need a
separate step to file it. A `capability_command` dry run returns
`requestStatus: "preview"` and records no event; rerun without `dryRun` to
create an approvable request.

### Example

```json
{
  "type": "permission_required",
  "status": "permission_required",
  "requestId": "local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b",
  "app": "notes-demo",
  "appName": "Notes Demo",
  "org": "local",
  "subject": "user:local-owner",
  "operation": "invoke:write",
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

## How to get unblocked (do not get stuck here)

An MCP client **cannot grant itself access.** The `capability_command` tool
refuses any `auth.*` name with `"<name> is trusted-admin-only; use the permission
request/admin approval flow"`. Do not retry `auth.grant` or
`auth.permission.approve` through `capability_command` — it will always fail.

Instead, pick one of the three trusted paths below, then **retry the original
`invoke`, `app_actions`, or direct resource `capability_command` call with the
same args** once the grant lands.

### (a) CLI grant — copy-paste from `grantCommands`

Run each string in `structuredContent.grantCommands` verbatim. Format:

```
terrane auth grant user:local-owner <app> <namespace>
```

For the example above:

```
terrane auth grant user:local-owner notes-demo kv
```

Arg order is `subject app namespace [verbs…]`. Verbs are optional; with none, the
namespace's full verb set is granted. There is no dedicated `auth` subcommand —
`auth grant …` (and `auth revoke …`, `auth agent.register …`, etc.) flows through
the generic `<ns> <verb> [args]` CLI path, which the CLI runs as a trusted host so
the command is admitted.

### (b) Admin UI — a human/admin approves

Open the URL from `adminUrl`:

```
http://127.0.0.1:8780/__terrane/admin                          (admin page)
http://127.0.0.1:8780/__terrane/admin/requests/<requestId>     (deep link)
```

Approving posts to `POST /__terrane/admin/requests/<requestId>/approve`, which
mints the missing grants and marks the request `approved`. The admin can also
grant directly via `POST /__terrane/admin/grants`. All admin control routes
require the trusted admin header `X-Terrane-Admin: local-admin` (otherwise
`403 "admin header required"`), and mutations are blocked while the local admin
session is locked. Approval is deliberately an admin/trusted action, not
something the requesting agent can self-serve.

### (c) MCP poll — wait for a human/admin to approve

If you are an MCP client, the recorded request is already `pending`. Your loop is:

1. A human/admin approves at `adminUrl`, or runs the `grantCommands`.
2. Poll `permission_check` with `{ "requestId": <requestId> }` until `status` is
   `approved`.
3. Retry `invoke` with the same args.

### (d) In-session approval — no restart (elicitation + loopback console)

The stdio `terrane-mcp` server can get a permission approved **against its live
Core**, so the grant is seen immediately with no restart:

- **Elicitation:** if the client declared the `elicitation` capability, a
  `permission_required` on `invoke`/`app_actions` becomes a server→client
  `elicitation/create` prompt. A **human** approves in the client UI; the server
  grants in-process (trusted) and retries the original call automatically. The
  model returns the real output, never a grant. Decline/timeout falls back to the
  `permission_required` flow above.
- **Loopback admin console:** the server also serves `TERRANE_ADMIN_ADDR`
  (default `127.0.0.1:8780`, `off` to disable): `GET /__terrane/admin/requests`,
  `POST /__terrane/admin/requests/<id>/approve|deny`. A same-machine operator
  (browser, curl, headless) approves against the same live Core.

Both channels are **human** actions carried over the server's back-channels, not
tools the model can call — the model still cannot self-grant.

**Single-writer lock.** This live-Core guarantee is enforced by an exclusive
advisory lock on `$TERRANE_HOME/log.bin`: only one process may hold a home for
writing at a time. So a stray `terrane auth grant` (path (a)) in a second
terminal is **refused** while a server holds the home — approve in-session (d) or
via the console, or stop the server first. Two independent writers can no longer
fork the state.

## The MCP permission tools

None of these grants access — approval is always a trusted admin action.

| Tool | Input | Returns |
|---|---|---|
| `permission_check` | `{ "requestId": "<id>" }` (required) | the request view as `structuredContent`, or text error `"permission request not found"` |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | cancels a pending request, returns the updated view (does not grant) |
| `permission_requests` | `{}` | `{ "requests": [ … ] }` — all local requests |

`permission_check` / `permission_requests` return a `PermissionRequestView` with
keys: `requestId`, `org`, `subject`, `app`, `appName`, `operation`, `source`,
`resumeTokenHash`, `resources[]` (each `{ namespace, selectorSchemaId,
resourceId, verbs[] }`), `status`, `adminUrl`, `decidedBy`, `decisionReason`.

Statuses to handle: `pending` | `approved` | `denied` | `cancelled` (the extra
`preview` and `unrecorded` values appear only on `PermissionRequired.requestStatus`;
`preview` means a dry run did not record a request).

## Grantable namespaces + verbs

| Namespace | Verbs |
|---|---|
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

`auth.grant` with no verbs argument grants the namespace's full verb set; an
explicit verbs argument is validated against the allowed set above. A namespace a
manifest requests but that is not in this registry is skipped, not blocked.

## Agent clamp / revoke

Because `auth.*` is trusted-host-only, an admin can also constrain non-owner
subjects (agents) beyond the initial grant. Grants can be **revoked**
(`auth revoke …`) and delegated agents can be **clamped** to a narrower subset of
the owner's grants — an agent can never exceed what its principal holds. These are
trusted-host commands on the same path as `auth.grant`; they are never reachable
through the MCP `capability_command` tool.

## App sandbox (same-origin)

Apps are isolated to their own origin/app id. A grant is scoped to a specific
`(org, subject, app, resource)` tuple, so granting `kv` to `notes-demo` does not
expose any other app's `kv` data, and one app cannot read another app's
resources. The reserved `__terrane/auth` KV projection (the grant/permission
state) is host-internal — it shares KV's physical backend but is not an app
namespace and is never handed to an app via `ctx.resource`.

## Weak-model recipe (copy-paste)

1. `workflows_list` → pick a workflow (e.g. `make_js_kv_app_no_filesystem`).
2. `app_scaffold` with `{ "id": "notes-demo", "name": "Notes Demo" }` (add
   `"withUi": true` for a page).
3. `app_register_inline` with `{ "files": <structuredContent.files>, "dryRun": true }`,
   then again without `dryRun` to commit.
4. `invoke` with `{ "app": "notes-demo", "verb": "write", "args": ["hello"] }`.
5. If the result has `"isError": true` and
   `structuredContent.type == "permission_required"`, the app's `kv` (etc.)
   namespace is ungranted. Do one of:
   - **Human/CLI**: run each string in `structuredContent.grantCommands`, e.g.
     `terrane auth grant user:local-owner notes-demo kv`.
   - **Admin UI**: open `structuredContent.adminUrl` and approve (trusted admin
     action).
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is `approved`.
6. Once granted/`approved`, **retry `invoke`** with the same args → success.
   (Do not try `capability_command` with an `auth.*` name — it is refused as
   trusted-admin-only.)

## Permission Model (tool exposure)

Clients should grant the model only the tools required for the task. A strong
locked-down app-building client can deny file reads, filesystem listing, shell,
grep, glob, web fetch, web search, and language-server tools. The MCP-only app
path still works through `app_scaffold` and `app_register_inline`.

## Mutation Rules

Mutating MCP tools must route through core dispatch or a host helper that
dispatches through core. They must not mutate capability state directly.

Examples:

- `app_register_inline` writes owned bundle files, then dispatches `app.add`.
- `app_register` validates a source bundle, then dispatches `app.add`.
- `capability_command` applies the public command policy before dispatching
  through core: `kv` / `crdt` / `relational_db` resource commands require the
  same grants as `invoke`, while `auth.*`, `kv.storage.*`, `app.import`,
  `app.remove`, `net.fetch`, `model.ask`, `harness.*`, and
  `js-runtime.run` / `wasm-runtime.run` are refused on the untrusted MCP path.

## Destructive Actions

Commands that remove apps, clear storage, fetch networks, run code, or write
runtime state should be treated as explicit operator actions. Prefer:

1. Read capability docs.
2. Call command help.
3. Dry-run when supported.
4. Commit only when the requested destructive action is explicit.

## Transport Notes

The stdio host writes protocol frames to stdout and diagnostics to stderr. The
HTTP host exposes the same MCP behavior at `POST /mcp` and reuses existing
origin/auth checks.

## Audit Expectations

Tool results should be structured and visible to the model. Tool-level failures
return `isError: true`; malformed protocol requests return JSON-RPC errors.
When possible, errors include a concrete next `tools/call` example. An ungranted
resource is one such failure: it returns `isError: true` with a
`permission_required` object whose `grantCommands`, `adminUrl`, and
`resumeTool` tell you exactly how to proceed.
