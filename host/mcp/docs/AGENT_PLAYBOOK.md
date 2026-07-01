# Terrane MCP Agent Playbook

This is the step-by-step playbook for any agent (any LLM) building or operating
Terrane apps over MCP. Follow it exactly and you will not get stuck. It is
written to stay reliable even for small, cheap, or locked-down models with little
or no prior knowledge of Terrane, but the same steps work for any model.

## Rules

- Start with `workflows_list`.
- Call `workflow_info` before acting.
- Prefer `app_register_inline` so no filesystem write/list/read tools are
  required.
- Parse `structuredContent` first; use text JSON only as a compatibility copy.
- Pass `app_register_inline.files` as the actual `structuredContent.files`
  array. Do not JSON-stringify it.
- Use `app_actions` before `invoke`.
- Expect the first `invoke` on a new app to return `permission_required`:
  resources are default-deny, so a manifest resource is not usable until granted.
  When it happens, see **When A Resource Is Denied (permission_required)** —
  never abandon the task over it.
- For UI apps, use `app_scaffold` with `withUi: true` and keep frontend code
  simple enough to parse.
- Do not use `capability_command app.add` while app-specific tools are
  available.
- After `app_scaffold`, the next assistant action should be a tool call:
  `app_register_inline` with `dryRun: true` and the complete
  `structuredContent.files` array. Do not pause to explain, summarize, or emit
  the whole app as prose/code before the dry run.
- If the client stops with `finish: "length"`, retry with a higher client output
  budget or a smaller first bundle. If the client stalls with no new output
  tokens after `app_scaffold`, restart from the scaffold result and call
  `app_register_inline` immediately.

## Minimal App Creation Path

1. `workflows_list`
2. `workflow_info` with `make_js_kv_app_no_filesystem`
3. `app_scaffold`
4. `app_register_inline` with `dryRun: true`
5. `app_register_inline` commit
6. `app_actions`
7. `invoke`
8. If `invoke` (or `app_actions`) returns `isError: true` with a
   `permission_required` object, follow **When A Resource Is Denied
   (permission_required)** below, then retry step 7.

## When A Resource Is Denied (permission_required)

**This is the most important section. Do not skip it. Do not give up when you
hit it.**

Declaring a resource in `manifest.json` (`kv`, `crdt`, `relational_db`, `build`)
does **not** grant it. Resources are **default-deny**: the manifest only
*requests* a namespace, and `ctx.resource.<ns>` is absent inside the app until
an admin grants it. So the **first** `invoke` on a fresh app usually comes back
denied. This is normal. Handle it, do not treat it as a dead end.

### Step 1 — Detect it

Only `invoke` and `app_actions` return this. It arrives as a tool result with:

```json
{
  "content": [{ "type": "text", "text": "<permission_required JSON as a string>" }],
  "structuredContent": { "type": "permission_required", "...": "..." },
  "isError": true
}
```

You have a denied resource when **both** are true:

- the result has `"isError": true`, and
- `structuredContent.type == "permission_required"`.

The `structuredContent` object has the fields you act on (it also carries
`type`/`status`/`org`/`subject`/`source`/`resumeTokenHash`):

| Field | What it is |
|---|---|
| `type` | always `"permission_required"` |
| `app` | the app id (e.g. `notes-demo`) |
| `missingResources` | list of ungranted namespaces, e.g. `["kv"]` |
| `grantCommands` | one ready-to-run CLI string per missing namespace |
| `adminUrl` | URL a human/admin opens to approve |
| `requestId` | id to poll with `permission_check` |
| `requestStatus` | `pending` \| `approved` \| `denied` \| `cancelled` \| `unrecorded` |
| `resumeTool` | always `"permission_check"` |
| `message` | human-readable one-liner |

### Step 2 — Get it granted (pick one)

You **cannot grant it yourself over MCP.** Granting is a trusted-admin action.
Choose whichever path fits who is present:

- **In-session approval (elicitation) — often nothing to do:** if your client
  supports MCP elicitation, the server prompts the operator to approve **inside
  your session** and, on approval, your original `invoke` **just succeeds** (the
  grant is applied to the live server, no restart). You may never see a
  `permission_required` at all. If it still comes back (the operator declined or
  the prompt timed out), use one of the paths below.

- **Human or CLI is available (fastest):** run each string in
  `structuredContent.grantCommands` verbatim in a terminal. The command format is
  exactly:

  ```
  terrane auth grant user:local-owner <app> <namespace>
  ```

  Worked example — if `missingResources` is `["kv"]` and the app is `notes-demo`:

  ```
  terrane auth grant user:local-owner notes-demo kv
  ```

  Run it once per entry in `missingResources`. No verbs argument is needed —
  omitting verbs grants the namespace's full verb set.

  Note: while a terrane server is running against this home, a **second**
  `terrane` process is refused (single-writer lock on the home). Prefer in-session
  or admin-console approval — those apply to the **live** server with no restart —
  or stop the server before running a CLI grant.

- **Admin console is available:** tell the human to open
  `structuredContent.adminUrl` (e.g.
  `http://127.0.0.1:8780/__terrane/admin/requests/<requestId>`) and approve — or,
  headless, `POST /__terrane/admin/requests/<requestId>/approve`. This approves
  against the live server (no restart). It is a trusted admin action; the
  requesting agent cannot self-serve it.

- **You are waiting on someone else to approve:** the denied result already
  **recorded** a pending request. Poll it:

  ```json
  { "tool": "permission_check", "arguments": { "requestId": "<structuredContent.requestId>" } }
  ```

  Repeat until the returned view's `status` is `approved`. Statuses you may see:
  `pending`, `approved`, `denied`, `cancelled`.

### Step 3 — Retry

Once granted (CLI ran, or admin approved, or `permission_check` shows
`approved`), **retry the exact same `invoke`** with the same `app`, `verb`, and
`args`. It now succeeds.

### Do NOT do this

- Do **not** call `capability_command` with an `auth.*` name (e.g. `auth.grant`,
  `auth.permission.approve`). Over MCP it is refused as trusted-admin-only. Use
  `grantCommands` / the admin URL / `permission_check` instead.
- Do **not** rewrite the app, change resources, or abandon the task because of a
  `permission_required` — the app is fine; it just needs a grant.
- Do **not** JSON-stringify or invent the grant command. Copy it from
  `grantCommands`.

### Grantable namespaces and their verbs

| Namespace | Verbs |
|---|---|
| `kv` | `read`, `write` |
| `crdt` | `read`, `write` |
| `relational_db` | `read`, `write` |
| `build` | `read` (read-only) |

The local subject is always `user:local-owner`.

## Choose A Workflow By Outcome

After `workflows_list`, match the user's requested outcome before picking a
recipe:

- Simple notes or single-resource key/value app:
  `make_js_kv_app_no_filesystem`
- Calendar, dashboard, form, natural-language input box, or other visible app
  backed by app state:
  `make_js_kv_app_no_filesystem` with `app_scaffold.withUi: true`
- App bundle already exists on disk:
  `register_app_bundle`
- Existing app must be operated:
  `inspect_app_actions`, then `run_app_action`
- Five-surface proof, multi-capability app, relational table, CRDT state, or
  replica identity:
  `make_js_multicap_app_no_filesystem`

If the outcome is unclear, call `app_recipe` after `workflow_info`. Recipes are
orientation helpers, not mutations. They help confirm scaffold kinds and expected
follow-up tools before you commit state.

## Post-Scaffold Rule

`app_scaffold` returns a ready-to-edit `files` array. Once that array exists, the
model has enough structure to validate through Terrane. The next output should
be:

```json
{
  "tool": "app_register_inline",
  "arguments": {
    "files": "structuredContent.files from app_scaffold",
    "dryRun": true
  }
}
```

For custom apps, modify the returned file contents inside that same array, then
send the full array. Do not print the files to the user first, do not send only
changed files, and do not switch to `capability_command app.add`.

After a successful dry run, call `app_register_inline` again with the same full
array and no `dryRun`. Only then move to `list_apps`, `app_actions`, and
`invoke`.

## UI App Contract

A UI app has `manifest.ui`, usually `index.html`. The host injects
`window.terrane.invoke` into that page.

Use this shape:

```js
await window.terrane.invoke("verb", "arg1", "arg2");
```

Each value after the verb is one backend string argument. Do not call
`window.terrane.invoke("verb", [arg1, arg2])` for two backend args; that sends
one argument.

For non-trivial pages, prefer:

- `index.html` for markup
- `style.css` for styling
- `ui.js` for browser behavior
- `main.js` for the backend `handle(input)`

Avoid one huge inline `<script>` in `index.html`; syntax errors there can make a
page look present while none of the app behavior works.

For optional KV/index keys, do not assume the key exists on first run. Copy this
pattern into generated `main.js` when a missing key should mean "empty":

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

Use that helper for index keys, optional settings, and cleanup code. It prevents
first-run reads from breaking an otherwise valid app.

For visible-app tasks, verify both layers:

- backend: `app_actions`, then `invoke` the verbs used by the UI
- UI: page loads, browser script parses, controls call the right verbs, and the
  rendered result matches the user's filter or view

If no browser or page tool is available, state that the UI was not live-tested
and keep the frontend code conservative.

## Harder Five-Capability Path

Use this benchmark when the model must prove it can follow richer docs. It
requires five capability surfaces: `app`, `kv`, `crdt`, `relational_db`, and
`replica`.

1. `workflows_list`
2. `workflow_info` with `make_js_multicap_app_no_filesystem`
3. `capability_info` for `kv`, `crdt`, `relational_db`, `replica`, and `app`
4. `app_scaffold` with `kind: "js_multicap_audit"`
5. `app_register_inline` with `dryRun: true`
6. `app_register_inline` commit
7. `capability_command` with `name: "replica.init", help: true`
8. `capability_command` with `name: "replica.init"`
9. `capability_query` for `replica.peer`
10. `capability_query` for `app.exists`
11. `app_actions`
12. `invoke seed`, `invoke summary`, `invoke clearKv`, `invoke summary`

This app requests three default-deny resources (`kv`, `crdt`, `relational_db`),
so the first `seed`/`invoke` may return `permission_required` with two or three
entries in `missingResources`. If it does, follow **When A Resource Is Denied
(permission_required)** and grant every namespace it lists — run one
`grantCommands` string per entry, e.g.:

```
terrane auth grant user:local-owner <app> kv
terrane auth grant user:local-owner <app> crdt
terrane auth grant user:local-owner <app> relational_db
```

Then retry the failed `invoke`.

Call `summary` as a separate read immediately after `seed` and again after
`clearKv`. `seed` and `clearKv` also return JSON, but mutation return values do
not replace the explicit pre-clear and post-clear reads.

Expected proof:

- `app.exists` returns true
- `replica.peer` returns a number. If `replica.init` reports `records:0`, that
  can still be success: the home was already initialized.
- `seed` returns JSON with `kv`, `crdt`, and `relational` sections
- the separate pre-clear `summary` after `seed` returns those same populated
  sections before any key/value clearing
- the separate final `summary` after `clearKv` shows KV note fields are null
  while CRDT and relational values remain

## Common Recovery

If a tool name is rejected, call `tools/list`.

If a workflow name is rejected, call `workflows_list`.

If app registration fails with missing files, use `app_register_inline` with the
complete `files` array from `app_scaffold`. A retry must include every file
referenced by `manifest.json`: `manifest.json`, backend, `manifest.ui`, `ui.js`,
`style.css`, and any other asset. Do not retry with only the changed file.

If app registration says `files` must be an array, you probably sent a JSON
string. Send the `structuredContent.files` array itself.

If app registration fails because the app id already exists:

- If the task is to use the existing app, call `app_actions`.
- If the task is to replace a broken generated app, stop and ask a trusted
  operator to remove or replace it out of band. Untrusted
  `capability_command app.remove` is refused; after the operator clears the app,
  rerun `app_register_inline`.

If the client ends before any `app_register_inline` call and the transcript shows
`finish: "length"`, the app was not rejected by Terrane. The model exhausted its
response budget while generating content. Resume with the same scaffold context,
send a compact complete bundle to `app_register_inline`, and continue from the
tool result.

If the client produces no new output tokens after `app_scaffold` and the session
database/log shows a new assistant turn with no finish reason, treat it as a
provider/client stall rather than a Terrane error. Restart or resume with the
same scaffold result and make `app_register_inline` dry-run the first tool call.

If a capability command is tempting, call `capability_command` with `help: true`
first and check whether a higher-level app tool exists.

If `invoke` or `app_actions` returns `isError: true` and
`structuredContent.type == "permission_required"`, the resource is denied, not
broken. Go to **When A Resource Is Denied (permission_required)**: run the
`grantCommands`, or have an admin approve at `adminUrl`, or poll `permission_check`
until `status` is `approved`, then retry the same `invoke`. Never try to grant it
yourself with `capability_command` and an `auth.*` name — that is refused.

The three permission tools (none of them grants access):

- `permission_check` with `{ "requestId": "<id>" }` — returns the current request
  view; poll its `status` for `approved`.
- `permission_requests` with `{}` — lists all local requests.
- `permission_cancel` with `{ "requestId": "<id>", "reason": "<text>" }` — cancels
  a pending request. Approval still remains a trusted admin action.

If `replica.init` returns `records:0`, call `capability_query` for
`replica.peer`. A numeric peer proves the identity already exists.

## Success Criteria

An agent run should prove:

- no source files were read
- no shell was used
- no broad filesystem listing was used
- the model created or registered an app
- `app_actions` exposed verbs
- if `invoke` returned `permission_required`, the model got the namespace granted
  (via `grantCommands` / admin approval / `permission_check`) and retried, rather
  than giving up or trying to grant itself via `capability_command`
- `invoke` produced the expected write/read/clear outputs
- for UI apps, the page itself was checked or the lack of browser verification
  was made explicit
