# Terrane MCP Guide

Terrane MCP is the agent-facing control surface for one local `TERRANE_HOME`.
It is exposed by the stdio host in this crate and by the web host at `POST /mcp`.
Both transports use the same shared implementation in `terrane-host`.

This directory owns the overall MCP manual:

- how clients connect
- which MCP concepts Terrane uses
- the app-building workflow
- guarded capability operation
- security and permission guidance (default-deny resources + grant handshake)
- constrained-model and no-source operation

Capability semantics do not live here. Each capability owns its own document in
its `terrane-cap-*` crate through `Capability::doc(include_internal)`. MCP serves
those capability-owned docs through `terrane://capabilities/{namespace}` and the
`capability_info` tool.

## MCP Model

Terrane uses three MCP surfaces:

- Tools are model-called actions. Use them for app registration, app invocation,
  capability queries, and guarded capability commands.
- Resources are read-only documentation and operational context. Use them for
  the host MCP manual, capability docs, workflow recipes, and app action docs.
- Prompts are user-invoked workflows. Use them to guide app creation, app
  registration, action inspection, and safe capability commands.

## Start Here

For app building:

1. Read `terrane://docs/app-building`.
2. Call `workflows_list`.
3. Call `workflow_info` for `make_js_kv_app_no_filesystem`.
4. Use `app_build_start`; pass `withUi: true` for visible apps such as
   calendars, dashboards, forms, and natural-language input pages.
5. Replace generated files one at a time with `app_build_put_file`.
6. Validate with `app_build_validate`.
7. Commit with `app_build_commit`.
8. Inspect with `app_actions`.
9. Act with `invoke`.

Step 9 can come back with `isError: true` and a `permission_required` object
instead of a result: the app declared a resource (such as `kv`) in its manifest,
but declaring a resource no longer auto-grants it — resources are default-deny.
Do not treat this as a dead end. See the [Permissions](#permissions) section
below for exactly what to do next.

If you use the older `app_scaffold` + `app_register_inline` bridge, pass
`app_register_inline.files` as the actual `structuredContent.files` array, not
as a JSON string. The dry-run returns `draftId` and `validationToken`; the next
call should be `app_build_commit`, without resending file contents. On dry-run
retry, include the complete bundle file array again, including `manifest.json`,
backend, UI, and assets.

For UI apps, the browser bridge is
`window.terrane.invoke("verb", "arg1", "arg2")`. Do not pass an array for
multiple backend arguments. Keep complex page behavior in `ui.js`, and verify
the page itself when the requested outcome is an interactive app.

For generated JS apps that maintain optional KV indexes, use a helper such as
`kvGetOrNull(kv, "event_ids")` and default missing index keys to an empty
array before parsing.

For capability operation:

1. Read `terrane://docs/capability-operations`.
2. Read `terrane://capabilities/{namespace}` or call `capability_info`.
3. Prefer `capability_query` for reads.
4. Use `capability_command` only after `help: true` and, when supported,
   `dryRun: true`.

## Permissions

Terrane resources are **default-deny**. An app's `manifest.json` only *requests*
a namespace (`kv`, `crdt`, `relational_db`, `build`); it does not grant it. Until
an admin grants that namespace to the executing subject for that app, the app
backend sees no `ctx.resource.<namespace>` methods, and `invoke` / `app_actions`
return an error instead of a result. Granting is a **trusted-admin-only** action:
an MCP client cannot grant itself. This is expected and recoverable — follow the
handshake below.

### Detecting a denied resource

`invoke`, `app_actions`, and grant-gated direct resource `capability_command`
calls return the denial as a tool result with `isError: true`. The payload is a
`permission_required` object, present both as `structuredContent` and as JSON
text in `content[0].text`:

```json
{
  "content": [{ "type": "text", "text": "<permission_required JSON>" }],
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

When you see `structuredContent.type == "permission_required"`, read
`missingResources`, `grantCommands`, `adminUrl`, `allowedMcpTools`,
`forbiddenMcpTools`, and `nextModelAction` — do not retry blindly and do not
give up. Surfacing this response also *records* a pending permission request, so
`requestStatus` becomes `pending` and it is immediately checkable and
approvable. If `operatorActionRequired` is true, the model's only MCP-side job is
to poll/cancel/list and retry after a trusted operator approves.

### Getting a grant (three exact paths)

The local subject is always `user:local-owner`. Grantable namespaces and their
verbs: `kv` (`read`, `write`), `crdt` (`read`, `write`), `relational_db`
(`read`, `write`), `build` (`read`, read-only).

1. **CLI** — run each string in `grantCommands` verbatim, one per missing
   namespace. The command form is:

   ```
   terrane auth grant user:local-owner <app> <namespace>
   ```

   Example: `terrane auth grant user:local-owner notes-demo kv`. Arg order is
   `subject, app, namespace, [verbs]`; omitting verbs grants the namespace's full
   verb set. The CLI is a trusted host, so this is allowed.

2. **Admin UI** — open `adminUrl` and approve. This is a trusted admin action on
   the web host and requires the admin surface (control routes require the
   `X-Terrane-Admin: local-admin` header). The requesting agent cannot approve on
   its own behalf.

3. **Poll from MCP** — call `permission_check` with
   `{ "requestId": structuredContent.requestId }` and wait until `status` is
   `approved`. A human or admin still performs the approval via path 1 or 2; MCP
   only observes it.

Then **retry the original `invoke`, `app_actions`, or direct resource
`capability_command` call** with the same args — it now succeeds.

### MCP permission tools

None of these grant access; they observe and manage requests.

- `permission_check` — input `{ "requestId": "<id>" }`; returns the request view
  (with `status`) or the text error `permission request not found`.
- `permission_cancel` — input `{ "requestId": "<id>", "reason"?: "<text>" }`;
  cancels a pending request. Approval still remains a trusted admin UI action.
- `permission_requests` — input `{}`; returns
  `{ "requests": [...] }` for all local requests.

Status values to handle: `pending`, `approved`, `denied`, `cancelled` (plus
`preview` and `unrecorded` only on `permission_required.requestStatus`; `preview`
means a dry run did not record a request).

### Do not

Do **not** try to grant via `capability_command` with an `auth.*` name (e.g.
`auth.grant`). The MCP `capability_command` tool refuses any `auth.*` command as
trusted-admin-only. Use the CLI `grantCommands`, the admin UI at `adminUrl`, or
`permission_check` polling instead.

For the full grant handshake with step-by-step recipes, see
`terrane://docs/agent-playbook` (`AGENT_PLAYBOOK.md`) and
`terrane://docs/app-building` (`APP_BUILDING.md`). For the security rationale
behind default-deny and trusted-admin approval, see `terrane://docs/security`
(`SECURITY.md`).

## Resource Index

- `terrane://docs/index`
- `terrane://docs/clients`
- `terrane://docs/app-building`
- `terrane://docs/capability-operations`
- `terrane://docs/security`
- `terrane://docs/agent-playbook`
- `terrane://capabilities/{namespace}`
- `terrane://workflows/{name}`
- `terrane://apps/{id}/actions`

## Boundary Rule

`host/mcp` explains how to operate Terrane over MCP. Capability crates explain
what a capability means, which commands and queries it owns, which events it
records, which app resources it grants, and what constraints apply.
