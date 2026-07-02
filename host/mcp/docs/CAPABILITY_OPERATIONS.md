# Terrane MCP Capability Operations

Direct capability operation is an advanced path. App-building should normally
use `app_build_start`, `app_build_put_file`, `app_build_validate`,
`app_build_commit`, `app_register`, `app_actions`, and `invoke`.

> **Resources are default-deny.** Declaring a resource in `manifest.json` only
> *requests* it — it does not grant it. `ctx.resource.<ns>` is **absent** inside
> an app backend until an admin grants that namespace. If `invoke`,
> `app_actions`, or a direct resource `capability_command` returns
> `"isError": true` with a `permission_required` object, the app's resource was
> withheld — do not treat it as a code bug. See
> [Default-Deny Permissions](#default-deny-permissions).

## Capability Docs

Capability docs are owned by capability crates, not by `host/mcp`.

Use one of:

```json
{"name":"capability_info","arguments":{"namespace":"kv","format":"json"}}
```

or:

```json
{"uri":"terrane://capabilities/kv"}
```

The capability doc is expected to include commands, queries, events, resource
methods, params, returns, errors, examples, limits, compatibility notes, and
internal notes when `includeInternal` is explicitly requested.

## Read Path

Use `capability_query` for reads.

```json
{
  "name": "capability_query",
  "arguments": {
    "capability": "app",
    "query": "exists",
    "args": ["notes-demo"]
  }
}
```

Queries must not append records, run effects, or touch runtime paths.

## Command Path

Use `capability_command` only after reading help.

```json
{
  "name": "capability_command",
  "arguments": {
    "name": "app.add",
    "help": true
  }
}
```

Then dry-run when supported.

```json
{
  "name": "capability_command",
  "arguments": {
    "name": "app.add",
    "args": ["notes-demo", "Notes Demo"],
    "dryRun": true
  }
}
```

Effect and runtime commands can reject dry-run. Treat that rejection as a guard,
not as a reason to force the command.

**`auth.*` commands are refused here.** `capability_command` explicitly rejects
any `auth.*` name (e.g. `auth.grant`, `auth.permission.approve`) with
`"<name> is trusted-admin-only; use the permission request/admin approval
flow"`. You cannot grant yourself a resource over MCP. Granting is a trusted
admin/CLI action — see [How to grant](#how-to-grant).

Direct `kv`, `crdt`, `relational_db`, and `local-model.ask` resource commands
are grant-gated by app id and can return `permission_required`. Raw storage
configuration (`kv.storage.*`), raw bundle import (`app.import`), app removal,
runtime execution, network, model, local model-spec management
(`local-model.register/pull/rm/default` — machine-local weights are
trusted-admin-only), and harness effect commands are refused on the public MCP
path.

## Default-Deny Permissions

A manifest that lists `kv`, `crdt`, `relational_db`, `build`, or
`local-model` under its
resources is only *requesting* those namespaces. Nothing is auto-granted. Until
an admin grants a namespace to the executing subject for that app, the app
backend sees no `ctx.resource.<ns>` methods for it.

### Grantable namespaces and verbs

| Namespace | Verbs | Notes |
|---|---|---|
| `kv` | `read`, `write` | |
| `crdt` | `read`, `write` | |
| `relational_db` | `read`, `write` | |
| `build` | `read` | **read-only** — no `write` verb exists |
| `local-model` | `call` | recorded local LLM generations (default or named model) |

`auth.grant` with no verbs argument grants the namespace's full verb set. An
explicit verbs argument is validated against the allowed set above. A namespace
a manifest requests that is not one of these is skipped, not blocked.

### The `permission_required` result

`invoke`, `app_actions`, and grant-gated direct resource `capability_command`
calls can return it. When they hit an ungranted requested namespace, the tool
result is an **error** (`"isError": true`) whose `structuredContent` is a
`permission_required` object. The same JSON is also copied as a string into
`content[0].text`.

```json
{
  "content": [
    { "type": "text", "text": "<the permission_required JSON as a string>" }
  ],
  "structuredContent": {
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
    "operatorActionRequired": true,
    "allowedMcpTools": ["permission_check", "permission_requests", "permission_cancel"],
    "forbiddenMcpTools": [
      "capability_command:auth.*",
      "capability_command:*.grant",
      "capability_command:app.grant",
      "capability_command:auth.permission.approve"
    ],
    "nextModelAction": "Do not call capability_command for auth/grant commands. Ask a trusted operator to approve adminUrl or run grantCommands, poll permission_check with requestId until approved, then retry the original invoke/app_actions/capability_command call with the same arguments.",
    "message": "permission required for app notes-demo: grant kv; open http://127.0.0.1:8780/__terrane/admin/requests/local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b"
  },
  "isError": true
}
```

Key fields to read:

- `missingResources` — the ungranted namespaces you need granted.
- `grantCommands` — one ready-to-run CLI command per missing namespace.
- `adminUrl` — deep link for an admin to approve.
- `requestId` — pass this to `permission_check` to poll status.
- `operation` — app/runtime verb or direct operation, e.g.
  `capability_command:kv.set`.
- `resumeTool` — `"permission_check"` for recorded requests; empty for dry-run
  previews.
- `requestStatus` — `pending` once a real denial is surfaced (the request is
  recorded as a side effect of returning this error), or `preview` for a
  `capability_command` dry run that did not record a request.
- `operatorActionRequired` — `true` means approval is outside model-callable MCP
  tools.
- `allowedMcpTools` — the only MCP tools the model should use while waiting,
  usually `permission_check`, `permission_requests`, and `permission_cancel`.
- `forbiddenMcpTools` — tool/command patterns the model must not call, such as
  `capability_command:auth.*`.
- `nextModelAction` — the exact model-side recovery instruction: surface
  approval paths, poll, then retry the original call.

The local subject is always `user:local-owner`.

### How to grant

Granting is a **trusted admin action**. An MCP client cannot self-serve it.
There are three ways, and the MCP client's job is to trigger a request and wait:

**(a) CLI — copy `grantCommands` verbatim.** Arg order is
`subject app namespace [verbs...]`:

```
terrane auth grant user:local-owner notes-demo kv
```

Verbs are optional and default to the namespace's full verb set. There is no
dedicated `auth` subcommand — any `auth.*` command flows through the generic
`<ns> <verb> [args...]` CLI path (e.g. `terrane auth revoke ...`). The CLI is a
trusted host, so it passes the grant gate.

**(b) Admin UI — open `adminUrl` and approve.** The admin page lives at
`http://127.0.0.1:8780/__terrane/admin`; the deep link
`http://127.0.0.1:8780/__terrane/admin/requests/<requestId>` opens it focused on
the request. Approving mints the missing grants and marks the request
`approved`. All admin control routes require the trusted admin header
`X-Terrane-Admin: local-admin` (else `403 "admin header required"`), and are
blocked while the local admin session is locked. Approval is explicitly an
admin/trusted action, not something the requesting agent can do.

**(c) MCP path — request, poll, retry.** The recorded `permission_required`
already created a `pending` request. Poll it with `permission_check`, wait for a
human/admin to approve at `adminUrl` (or run the `grantCommands`), then retry
the original `invoke`, `app_actions`, or direct resource `capability_command`
call. Do **not** call `capability_command` with an `auth.*` name — it is refused
as trusted-admin-only.

### MCP permission tools

None of these grant access.

| Tool | Input | Returns |
|---|---|---|
| `permission_check` | `{ "requestId": "<id>" }` (required) | The `PermissionRequestView` for that request as `structuredContent`, or text error `"permission request not found"`. |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | Cancels a pending request, returns the updated view. Does **not** grant — approval remains a trusted admin UI action. |
| `permission_requests` | `{}` | `{ "requests": [PermissionRequestView, ...] }` — all local requests. |

`PermissionRequestView` fields: `requestId`, `org`, `subject`, `app`, `appName`,
`operation`, `source`, `resumeTokenHash`, `resources[]` (each
`{ namespace, selectorSchemaId, resourceId, verbs[] }`), `status`, `adminUrl`,
`decidedBy`, `decisionReason`.

Status values to handle: `pending`, `approved`, `denied`, `cancelled` (plus
`preview` and `unrecorded`, which appear only on
`permission_required.requestStatus`; `preview` means a dry run did not record a
request).

### Worked example: unblocking a denied `invoke`

1. `invoke` with `{ "app": "notes-demo", "verb": "write", "args": ["hello"] }`.
2. Result has `"isError": true` and
   `structuredContent.type == "permission_required"` — the app's `kv` namespace
   is ungranted.
3. Get it granted (pick one):
   - **Human/CLI**: run each string in `structuredContent.grantCommands`, e.g.
     `terrane auth grant user:local-owner notes-demo kv`.
   - **Admin UI**: open `structuredContent.adminUrl` and approve (trusted admin
     action).
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is
     `approved`.
4. Once granted/`approved`, **retry `invoke`** with the same args → success.

Do not try `capability_command` with an `auth.*` name to shortcut this — it is
refused.

## Safer Alternatives

- Use `app_build_*` or `app_register_inline` instead of raw `app.add` for
  generated app files.
- Use `app_register` instead of raw `app.add --source` for existing bundles.
- Use `app_actions` and `invoke` instead of runtime capability commands.
- Use `app_register*` instead of raw `app.import`; raw import is refused on the
  public MCP path because it can configure storage.
- Use `capability_query` instead of commands for state inspection.
- To unblock a `permission_required` result, use `permission_check` and the
  `grantCommands` / `adminUrl` from the response — never
  `capability_command` with an `auth.*` name (it is refused).
