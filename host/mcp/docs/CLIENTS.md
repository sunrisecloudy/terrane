# Terrane MCP Clients

Terrane MCP can run over stdio or HTTP.

## Stdio

Use the `terrane-mcp` binary from `host/mcp`.

```json
{
  "mcpServers": {
    "terrane": {
      "command": "/absolute/path/to/terrane-mcp",
      "env": {
        "TERRANE_HOME": "/absolute/path/to/.terrane"
      }
    }
  }
}
```

The stdio host reads newline-delimited JSON-RPC from stdin and writes only
protocol responses to stdout. Diagnostics go to stderr.

## HTTP

The web host exposes the same MCP implementation at `POST /mcp`.

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list"}
```

Loopback binds do not require auth. Non-loopback binds require the web host's
bearer-token auth and existing origin checks.

## Resources Are Default-Deny — Handling `permission_required`

**Read this before building or running any app.** This is the single most common
place a client gets stuck.

Declaring a resource in `manifest.json` (`kv`, `crdt`, `relational_db`, `build`)
**does not grant it**. Resources are **default-deny**: the manifest only *requests*
a namespace. Inside the app backend, `ctx.resource.<ns>` is **absent** until an
admin grants that namespace to the executing subject for that app. With no grant,
the app simply cannot see the resource methods.

So the very first time you `invoke` (or `app_actions`) an app that touches an
ungranted namespace, you will get an **error result**, not a crash. Do not treat
it as failure — it is a handshake. Handle it, then retry.

### What the error looks like

`invoke`, `app_actions`, and grant-gated direct resource `capability_command`
calls return this. The result has `"isError": true` and carries a
`permission_required` object **both** in `structuredContent` and as a JSON string
in `content[0].text`:

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

Detect it with exactly:

```
result.isError === true && result.structuredContent.type === "permission_required"
```

Surfacing this error also **records** a pending permission request as a side
effect, so `requestStatus` is `pending` and the request is immediately listable,
pollable, and approvable. You do not need to create the request yourself.

### The fields you must read

| Field | Use it for |
|---|---|
| `missingResources` | the namespaces that need granting, e.g. `["kv"]` |
| `grantCommands` | ready-to-run CLI commands, one per missing namespace |
| `adminUrl` | deep link a human/admin opens to approve |
| `requestId` | pass to `permission_check` / `permission_cancel` to poll or cancel |
| `operation` | app/runtime verb or direct operation, e.g. `capability_command:kv.set` |
| `resumeTool` | `"permission_check"` for recorded requests; empty for dry-run previews |
| `requestStatus` | `pending` \| `approved` \| `denied` \| `cancelled` \| `preview` \| `unrecorded` |
| `operatorActionRequired` | `true` means a trusted operator/admin must grant |
| `allowedMcpTools` | MCP tools the model may call while waiting |
| `forbiddenMcpTools` | MCP tool/command patterns the model must not call |
| `nextModelAction` | exact recovery instruction for the model |
| `message` | one-line human-readable summary |

The local subject is always **`user:local-owner`**.

### What to do next

You (the MCP client) **cannot grant yourself access.** Approval is a trusted
admin action. Your job is to surface the request and then poll and retry. Do
**one** of these to get the grant, then retry the original call:

If present, follow `nextModelAction` before any free-form text. Treat
`allowedMcpTools` as the safe waiting set and `forbiddenMcpTools` as hard
refusals; do not try to turn a CLI grant command into a
`capability_command` call.

1. **CLI (human or a shell you control).** Run each string in `grantCommands`
   verbatim. The format is always:

   ```sh
   terrane auth grant user:local-owner <app> <namespace>
   ```

   Example, straight from the payload above:

   ```sh
   terrane auth grant user:local-owner notes-demo kv
   ```

   (Arg order is `subject app namespace [verbs…]`; omit verbs to grant the
   namespace's full verb set.)

2. **Admin UI (trusted admin).** Open `adminUrl` in a browser and approve. The
   admin page is `http://127.0.0.1:8780/__terrane/admin`; the deep link
   `.../admin/requests/<requestId>` opens the same page focused on this request.
   Admin routes require the `X-Terrane-Admin: local-admin` header, so only the
   admin surface can approve — the requesting agent cannot self-serve.

3. **Poll, then retry.** Regardless of which approval path a human uses, call
   `permission_check` with the `requestId` until `status` is `approved`, then
   **retry the original `invoke`, `app_actions`, or direct resource
   `capability_command` call with the same args** — it now succeeds.

> Do **not** try `capability_command` with an `auth.*` name to grant yourself.
> It is refused as trusted-admin-only: `"<name> is trusted-admin-only; use the
> permission request/admin approval flow"`. Granting only works through a trusted
> host (the CLI or the web admin UI with the admin header).

### The permission tools (none of these grant access)

| Tool | Input | Returns |
|---|---|---|
| `permission_check` | `{ "requestId": "<id>" }` | the request's current view; `status` tells you `pending`/`approved`/`denied`/`cancelled`. Error text `"permission request not found"` if the id is unknown. |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | cancels a pending request and returns the updated view. Does not grant. |
| `permission_requests` | `{}` | `{ "requests": [ … ] }` — every local permission request. |

`permission_check` / `permission_requests` return a `PermissionRequestView` with:
`requestId`, `org`, `subject`, `app`, `appName`, `operation`, `source`,
`resumeTokenHash`, `resources[]` (each `{ namespace, selectorSchemaId,
resourceId, verbs[] }`), `status`, `adminUrl`, `decidedBy`, `decisionReason`.

### Grantable namespaces and verbs

| Namespace | Verbs |
|---|---|
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

A namespace a manifest requests but that is not in this registry is skipped (not
blocked). `terrane auth grant … <namespace>` with no verbs grants the full set
above; an explicit verbs argument is validated against the allowed set.

### End-to-end recipe (copy-paste)

1. `workflows_list` → pick a workflow (e.g. `make_js_kv_app_no_filesystem`).
2. `app_build_start` with `{ "id": "notes-demo", "name": "Notes Demo" }`
   (add `"withUi": true` for a page).
3. `app_build_put_file` for each generated file you change, one file per call.
4. `app_build_validate` with `{ "draftId": <draftId> }`.
5. `app_build_commit` with `{ "draftId": <draftId>, "validationToken": <token> }`.
6. `invoke` with `{ "app": "notes-demo", "verb": "write", "args": ["hello"] }`.
7. If the result is `isError:true` with
   `structuredContent.type == "permission_required"`, the app's `kv` (etc.)
   namespace is ungranted. Get it approved:
   - **Human/CLI**: run each string in `structuredContent.grantCommands`,
     e.g. `terrane auth grant user:local-owner notes-demo kv`.
   - **Admin UI**: open `structuredContent.adminUrl` and approve.
   - **Poll**: call `permission_check` with
     `{ "requestId": structuredContent.requestId }` until `status` is `approved`.
8. **Retry `invoke`** with the same args → success.

## Opencode Locked-Agent Pattern

For no-source tests, configure Terrane as a native MCP server and deny file and
shell tools in the agent. The model should still be able to create an app using
`app_build_start`, `app_build_put_file`, `app_build_validate`, and
`app_build_commit`. If it uses the older `app_scaffold` +
`app_register_inline` bridge, the dry-run returns `draftId` and
`validationToken`; the next call should be `app_build_commit`.

Useful denies:

```yaml
permission:
  read: deny
  list: deny
  glob: deny
  grep: deny
  bash: deny
  webfetch: deny
  websearch: deny
  lsp: deny
  skill: deny
```

The locked agent should use only Terrane MCP tools and, if the client allows it,
MCP resources and prompts.

For eval runs, also remove opencode's smaller default output request budget. The
model/provider still has a real maximum, but the client should request the
model's advertised output limit:

```json
{
  "plugin": [
    "/absolute/path/to/terrane/host/mcp/evals/opencode/max-output-budget.mjs"
  ]
}
```

The plugin can be combined with the MCP config in the same `.opencode/opencode.json`.
Without it, long code-generation turns can stop with `finish: "length"` before
the model calls `app_build_validate` or the compatibility
`app_register_inline` dry-run.

### Opencode Eval Diagnostics

When a locked opencode run fails, classify it from opencode state before
changing Terrane docs or tools.

Useful local paths:

- Session DB: `~/.local/share/opencode/opencode.db`
- Log file: `~/.local/share/opencode/log/opencode.log`
- Eval home: the `TERRANE_HOME` configured in the temp `.opencode/opencode.json`

Find the session from the run title, then inspect finish reasons and tool calls:

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select id,title,tokens_input,tokens_output,tokens_reasoning,cost,time_updated
   from session
   where title like '%Calendar app product request%'
   order by time_created desc
   limit 5;"
```

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select id,
          json_extract(data,'$.role'),
          json_extract(data,'$.finish'),
          json_extract(data,'$.tokens.input'),
          json_extract(data,'$.tokens.output'),
          json_extract(data,'$.tokens.reasoning'),
          time_updated
   from message
   where session_id='SESSION_ID'
   order by time_created,id;"
```

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select json_extract(data,'$.type'),
          json_extract(data,'$.tool'),
          json_extract(data,'$.state.status'),
          length(json_extract(data,'$.text'))
   from part
   where session_id='SESSION_ID'
   order by time_created,id;"
```

Read the results this way:

- `finish = "length"` before `app_build_validate` or `app_register_inline`:
  client/provider output budget, not a Terrane MCP rejection.
- A final assistant message with `output = 0`, empty `finish`, and no new tool
  parts after `app_build_start`: provider/client stall. Restart from the
  draft id and call `app_build_get`, then continue with `app_build_put_file` or
  `app_build_validate`.
- A final assistant message with `output = 0`, empty `finish`, and no new tool
  parts after `app_scaffold`: provider/client stall. Restart from the scaffold
  result and make `app_register_inline` dry-run the first call, then commit with
  `app_build_commit`.
- `app_build_validate` or `app_register_inline` returns `isError: true`:
  Terrane rejected the bundle; keep the error and fix the draft or complete
  files array.
- App files absent under `$TERRANE_HOME/apps/<id>` after a failed run: the model
  never committed registration or registration was rejected.

For locked-agent app-building evals, a good first watchdog is: if the session DB
has not changed for five minutes after a `stream` line in `opencode.log`, poll
once more, then stop and classify the run from DB/tool evidence.

## Raw JSON-RPC Smoke

Initialize:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}
```

List tools:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

Read docs:

```json
{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"terrane://docs/app-building"}}
```

Get a prompt:

```json
{"jsonrpc":"2.0","id":4,"method":"prompts/get","params":{"name":"make_js_kv_app","arguments":{"id":"notes-demo","name":"Notes Demo"}}}
```

Invoke an app verb (may return a `permission_required` result with `isError:true` —
see "Resources Are Default-Deny" above):

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"invoke","arguments":{"app":"notes-demo","verb":"write","args":["hello"]}}}
```

Poll a pending permission request by its `requestId`:

```json
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"permission_check","arguments":{"requestId":"local-notes-demo-user-local-owner-kv-1a2b3c4d5e6f7a8b"}}}
```
