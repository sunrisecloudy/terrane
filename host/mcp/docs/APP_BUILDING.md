# Terrane MCP App Building

The primary MCP use case is building and operating apps. The preferred route is
MCP-only and does not require source-code access or shell access.

## Resources Are Default-Deny (Read This First)

Declaring a resource in `manifest.json` (`kv`, `crdt`, `relational_db`,
`build`) **no longer auto-grants it**. Resources are **default-deny**: the
manifest only *requests* a namespace. Inside the app backend,
`ctx.resource.<ns>` is **absent** until an admin grants that namespace to the
executing subject for that app. Registering an app is therefore not enough to
run it — there is a **grant step** between install and a successful `invoke`.

Only two tools surface this: `invoke` and `app_actions`. When you run either
against an app with an ungranted requested namespace, the result comes back with
`"isError": true` and a `permission_required` object (see
[Handling `permission_required`](#handling-permission_required)). This is
expected, not a failure of your app — do not remove or rewrite the app. Do the
grant, then retry the same call.

An MCP client **cannot grant itself**. Granting is a trusted-admin action
performed via the CLI, the admin UI, or by a human. Your job as an MCP client is
to (1) trigger the request by invoking, (2) surface the `grantCommands` /
`adminUrl` so an admin can approve, and (3) poll `permission_check`, then retry.

## Preferred MCP-Only Flow

1. Call `app_recipe` with `{ "kind": "js_kv_app" }`.
2. Call `app_build_start` with an app id and display name.
3. Use `app_build_put_file` for each file you change, one file at a time.
4. Call `app_build_validate`.
5. Call `app_build_commit` with the returned `draftId` and `validationToken`.
6. Call `list_apps`.
7. Call `app_actions` for the new app id.
8. Call `invoke` with a verb documented by `app_actions`.
9. **If the result is `"isError": true` with a `permission_required` object,**
   the app's requested namespace (e.g. `kv`) is not yet granted. Grant it (CLI,
   admin UI, or human), then repeat step 8. See
   [Handling `permission_required`](#handling-permission_required).

`app_build_commit` writes the app bundle under `TERRANE_HOME/apps/<id>` only
after validation. It still dispatches `app.add` through core, so catalog
mutation uses the normal capability path.

The older `app_scaffold` + `app_register_inline` route is still supported.
`app_register_inline` dry-run now returns `draftId` and `validationToken`, so
the recommended next call is `app_build_commit` rather than resending the same
large files array.

For visible apps, pass `withUi: true` to `app_build_start` (or to
`app_scaffold` on the compatibility route), or include a `manifest.ui` file
yourself. Keep browser code in a small `index.html` plus a separate asset such
as `ui.js` when the UI is more than a trivial button. Inline HTML scripts are
easy for small models to break.

If a stall or restart loses your `draftId`, call `app_build_list`: it returns
every draft with its app id and file summaries, newest first, so you can resume
with `app_build_get`, `app_build_put_file`, or `app_build_validate` instead of
starting over.

## Backend And Manifest Contract

These three contracts are where builds fail after discovery. Follow them
exactly; `app_build_validate` enforces the first two and returns fix-it errors.

**Backend contract.** `main.js` runs as ONE plain script in the app runtime:

- No top-level `import`/`export`, no `require`, no modules, no Deno or Node
  APIs. Inline everything.
- Define one global `function handle(input)` (or declare an `actions` table —
  the runtime synthesizes `handle` from it). Do not use `const handle = ...`;
  the runtime reads `handle` from the global object.
- `input` is an **array of strings**: `input[0]` is the verb, `input.slice(1)`
  are the args. Never read `input.action`, `input.verb`, or `input.args`.
- Return a **string**. Use `JSON.stringify(...)` for structured results.
- Storage is `ctx.resource.kv` (`get`/`set`/`rm`/`scan`/...); wrap `kv.get` in
  try/catch because missing keys throw.

**Manifest contract.** `manifest.json` is exactly this shape:

```json
{"id":"my-app","name":"My App","runtime":"js","backend":"main.js","ui":"index.html","resources":["kv"]}
```

`ui` is a **string file path** — omit it for backend-only apps, and never use
an object such as `{"index": "...", "scripts": [...]}`. Scripts and styles are
referenced from `index.html`, not listed in the manifest.

**UI contract.** Browser code calls
`window.terrane.invoke("verb", "arg1", "arg2")` with positional string args and
awaits the backend's string reply. Do not pass an args array or an object.

## Choosing A Flow

Start with `workflows_list` when the user describes an outcome rather than a
tool. Pick the workflow whose summary matches the requested result:

- key/value notes app: `make_js_kv_app_no_filesystem`
- visible calendar, dashboard, form, or natural-language UI backed by app state:
  `make_js_kv_app_no_filesystem` with `app_build_start.withUi: true`
- existing bundle path: `register_app_bundle`
- existing app operation: `inspect_app_actions`
- multi-capability proof with KV, CRDT, relational data, and replica identity:
  `make_js_multicap_app_no_filesystem`

`app_recipe` is a read-only orientation helper. Use it after choosing a workflow
when you need scaffold-kind or manifest-resource confirmation.

## Existing Bundle Flow

Use this when the client already has a bundle directory.

1. Call `app_bundle_validate` with the bundle path.
2. Call `app_register` with `dryRun: true`.
3. Call `app_register` again without `dryRun`.
4. Confirm with `list_apps`.
5. Discover with `app_actions`.

## App Runtime Contract

A JS app has a `manifest.json` and a backend file such as `main.js`.

```json
{
  "id": "notes-demo",
  "name": "Notes Demo",
  "runtime": "js",
  "backend": "main.js",
  "resources": ["kv"]
}
```

`manifest.resources` is a **request list, not a grant**. Listing `"kv"` asks the
host for the `kv` namespace; it does not turn it on. Until an admin grants `kv`
for this app and subject, `ctx.resource.kv` is undefined inside the backend and
the host returns a `permission_required` error instead of running the app. See
[Resources Are Default-Deny](#resources-are-default-deny-read-this-first).

Grantable namespaces and their verbs (namespaces requested but not in this table
are silently skipped, not blocked):

| Namespace | Verbs |
|---|---|
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

The backend exposes `handle(input)`. `input[0]` is the verb and the rest are
string arguments. Apps should implement `__actions__` so MCP clients can inspect
verbs before invoking them.

The web/macOS UI bridge is:

```js
await window.terrane.invoke("verb", "arg1", "arg2");
```

Each argument after the verb becomes one backend string argument. Do not pass an
array unless the backend expects one JSON/string argument. For example,
`window.terrane.invoke("range", start, end)` reaches `input[1]` and `input[2]`;
`window.terrane.invoke("range", [start, end])` reaches only `input[1]` as one
string.

For optional KV/index reads in JS apps, use a small helper instead of assuming
the key already exists:

```js
function kvGetOrNull(kv, key) {
  try {
    return kv.get(key);
  } catch (err) {
    if (String(err).indexOf("not found") !== -1) return null;
    throw err;
  }
}

var ids = JSON.parse(kvGetOrNull(kv, "event_ids") || "[]");
```

This is especially important for index keys such as `event_ids`, first-run
state, and cleanup flows. Initialize index keys before relying on them, and use
the same helper anywhere a missing key should mean "empty".

When a UI app is part of the requested outcome, backend `invoke` checks are not
enough. Verify the app page loads, browser script parses, a UI control calls the
expected backend verb, and the displayed result matches the requested filter or
view. If a browser is unavailable, keep the UI code simple, report the limitation
explicitly, and still verify the backend verbs through `app_actions` and
`invoke`.

Capability-specific app resources, such as `ctx.resource.kv`, are documented by
the owning capability. Read `terrane://capabilities/kv` for KV semantics,
constraints, errors, and examples. Remember: `ctx.resource.kv` only exists after
`kv` is granted for this app.

## Handling `permission_required`

When you `invoke` (or call `app_actions`) against an app whose requested
namespace is not yet granted, the tool result is an **error**: `"isError": true`,
with the same object in `structuredContent` and as a JSON string in
`content[0].text`.

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

Detect it by checking `structuredContent.type == "permission_required"` (or
`status == "permission_required"`). The fields you act on:

- `missingResources` — the namespaces that need granting.
- `grantCommands` — one ready-to-run CLI command per missing namespace.
- `adminUrl` — the admin page / deep link for a human to approve.
- `requestId` — pass this to `permission_check` to poll status.
- `operation` — app/runtime verb or direct operation, e.g.
  `capability_command:kv.set`.
- `resumeTool` — `permission_check` for recorded requests; empty for dry-run
  previews.
- `operatorActionRequired` — `true` means the model needs trusted operator/admin
  approval, not another grant tool call.
- `allowedMcpTools` — MCP tools safe to use while waiting, usually
  `permission_check`, `permission_requests`, and `permission_cancel`.
- `forbiddenMcpTools` — MCP tool/command patterns not to call, especially
  `capability_command:auth.*` and grant/approve attempts.
- `nextModelAction` — explicit recovery instruction for the model: surface
  approval paths, poll, then retry the original call.

Surfacing this error **records a pending request** as a side effect, so the
request is immediately listable and approvable — you do not need a separate
"create request" call.

### Do NOT try to grant from MCP

The MCP `capability_command` tool **refuses** any `auth.*` name with
`"<name> is trusted-admin-only; use the permission request/admin approval flow"`.
There is no MCP tool that grants a namespace. Granting is always a trusted-admin
action. The MCP permission tools below only *inspect* or *cancel* requests — none
of them grants access:

| Tool | Input | Purpose |
|---|---|---|
| `permission_check` | `{ "requestId": "<id>" }` | Returns the request view; poll until `status` is `approved`. If `status` becomes `denied` or `cancelled`, **stop polling** — the grant will not arrive; surface it to the human rather than retrying `invoke`. |
| `permission_cancel` | `{ "requestId": "<id>", "reason"?: "<text>" }` | Cancels a pending request. Does not grant. |
| `permission_requests` | `{}` | Lists all local requests (`{ "requests": [...] }`). |

`permission_check` returns a `PermissionRequestView` whose `status` is one of
`pending`, `approved`, `denied`, or `cancelled`. Retry `invoke` only once
`status` is `approved`.

### Three ways to grant (all trusted-admin)

**(a) CLI — run the strings in `grantCommands` verbatim.** The command shape is:

```
terrane auth grant user:local-owner <app> <namespace>
```

Arg order is `subject app namespace [verbs...]`. Omitting `verbs` grants the
namespace's full verb set (e.g. `read write` for `kv`). The local subject is
always `user:local-owner`. Example from the object above:

```
terrane auth grant user:local-owner notes-demo kv
```

The CLI is a trusted host, so `auth grant` is admitted. There is no dedicated
`auth` subcommand — any `auth.*` command (`auth grant`, `auth revoke`,
`auth agent.register`) flows through the generic `<ns> <verb> [args...]` path.

**(b) Admin UI — open `adminUrl`.** The admin surface lives at
`http://127.0.0.1:8780/__terrane/admin`, and the deep link is
`http://127.0.0.1:8780/__terrane/admin/requests/<requestId>`. An admin approves
there (which mints the missing grants and marks the request `approved`), or
grants directly. Admin control routes require the trusted header
`X-Terrane-Admin: local-admin` or they return `403 "admin header required"`, so
this is genuinely an admin action, not something the requesting agent can
self-serve.

**(c) Human hand-off.** When neither CLI nor admin surface is available to you,
report the `message`, the `grantCommands`, and the `adminUrl` to the user and ask
them to approve, then poll `permission_check`.

## Worked Example: A KV App End To End

This is the full path for an app that needs `kv`, including the grant step.

1. Orient (optional): `workflows_list`, then pick
   `make_js_kv_app_no_filesystem`.
2. Scaffold:

   ```json
   app_build_start { "kind": "js_kv_app", "id": "notes-demo", "name": "Notes Demo" }
   ```

   The generated `manifest.json` includes `"resources": ["kv"]` — a request,
   not a grant.
3. Replace any generated files you customized:

   ```json
   app_build_put_file { "draftId": <draftId>, "path": "main.js", "content": "<complete main.js>" }
   ```
4. Validate the server-side draft:

   ```json
   app_build_validate { "draftId": <draftId> }
   ```
5. Commit the validated draft:

   ```json
   app_build_commit { "draftId": <draftId>, "validationToken": <token> }
   ```
6. Inspect verbs: `app_actions { "app": "notes-demo" }`.
7. Invoke a verb:

   ```json
   invoke { "app": "notes-demo", "verb": "write", "args": ["hello"] }
   ```
8. **First invoke returns `"isError": true`** with
   `structuredContent.type == "permission_required"` and
   `missingResources: ["kv"]`. This is expected on first run. Do **not** delete
   or rebuild the app.
9. Grant `kv` (pick one):
   - CLI: run each string in `grantCommands`, e.g.
     `terrane auth grant user:local-owner notes-demo kv`.
   - Admin UI: open `structuredContent.adminUrl` and approve.
   - Poll: call `permission_check { "requestId": <requestId> }` until
     `status` is `approved`.
10. **Retry the exact same invoke** — now it succeeds:

   ```json
   invoke { "app": "notes-demo", "verb": "write", "args": ["hello"] }
   ```

If you get stuck at step 7 with no way to grant, that is the moment to hand off
to a human with the `grantCommands` and `adminUrl` — not to modify the app.

## Replacing A Generated App

If registration fails because the app id already exists, first decide whether
the task is to operate the existing app or replace a failed generated app.

- To operate it, call `list_apps`, `app_actions`, then `invoke`.
- To replace it, stop and ask a trusted operator to remove or replace the app out
  of band. Untrusted `capability_command app.remove` is refused. After the
  operator clears the app, rerun `app_build_validate`/`app_build_commit` if you
  still have a draft, or rerun `app_register_inline` dry-run if you only have a
  scaffolded files array.

Do not remove an existing app just because registration failed; only replace
when the current task is explicitly creating a new version of that app.

When retrying `app_register_inline`, send the complete files array every time:
`manifest.json`, the backend file, every `manifest.ui` file, and referenced
assets such as `ui.js` and `style.css`. Prefer `app_build_put_file` for a
server-side draft when only one file changed. Do not send only the changed file
through `app_register_inline`.

## Multi-Cap App Flow

Use this when a task must prove more than a simple KV app. The scaffold kind
`js_multicap_audit` generates a backend JS app with:

- manifest resources: `kv`, `crdt`, and `relational_db`
- app actions: `seed`, `summary`, and `clearKv`
- self-described capability coverage: `app`, `kv`, `crdt`, `relational_db`,
  and `replica`

MCP-only sequence:

1. Call `workflow_info` with `{ "name": "make_js_multicap_app_no_filesystem" }`.
2. Call `app_build_start` with `{ "kind": "js_multicap_audit", "id": "...", "name": "..." }`.
3. Customize generated files with `app_build_put_file` as needed.
4. Validate with `app_build_validate`.
5. Commit with `app_build_commit`.
6. Call `capability_command` with `{ "name": "replica.init", "help": true }`.
7. Call `capability_command` with `{ "name": "replica.init" }`.
8. Call `capability_query` for `replica.peer` and `app.exists`.
9. Call `app_actions`, then invoke `seed`, `summary`, `clearKv`, and `summary`.

Because this app requests `kv`, `crdt`, **and** `relational_db`, the first
`invoke` (or `app_actions`) reports `permission_required` with **all** ungranted
namespaces in `missingResources`. Grant every one before the app runs — the
`grantCommands` array already contains one command per missing namespace, e.g.:

```
terrane auth grant user:local-owner <app> kv
terrane auth grant user:local-owner <app> crdt
terrane auth grant user:local-owner <app> relational_db
```

The generated app keeps `replica` outside `ctx.resource`: replica identity is an
operator/core capability checked through the direct MCP tools above, so it is not
part of the grant flow. Once granted, app code uses `ctx.resource.kv`,
`ctx.resource.crdt`, and `ctx.resource.relational_db`.

For evaluation-style tasks, always call `summary` separately after `seed` and
again after `clearKv`. The `seed` and `clearKv` actions return summaries too,
but those mutation returns do not replace the explicit pre-clear and post-clear
reads.

## Do Not

- Do not call `capability_command app.add` as the first app-building path.
- Do not register a bundle before validation or dry-run.
- Do not pass `files` as a JSON string to `app_register_inline`.
- Do not retry registration with only changed files; always send the complete
  files array.
- Do not invoke a verb before calling `app_actions`.
- Do not assume filesystem tools are available.
- Do not count a UI app complete from backend invokes alone when the requested
  outcome is an interactive page.
- Do not treat `manifest.resources` as a grant; it is only a request. Expect a
  `permission_required` error on the first `invoke` of a fresh app.
- Do not delete, rewrite, or re-register an app just because `invoke` returned
  `permission_required`. Grant the namespace and retry the same call.
- Do not try to grant via MCP (`capability_command` with an `auth.*` name is
  refused). Use `grantCommands`, the `adminUrl`, or hand off to a human.
- Do not give up when a resource is denied. Follow the handshake: surface
  `grantCommands`/`adminUrl`, poll `permission_check` until `approved`, retry.
