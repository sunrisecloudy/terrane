# Terrane MCP App Building

The primary MCP use case is building and operating apps. The preferred route is
MCP-only and does not require source-code access or shell access.

## Preferred MCP-Only Flow

1. Call `app_recipe` with `{ "kind": "js_kv_app" }`.
2. Call `app_scaffold` with an app id and display name.
3. Take `structuredContent.files` from `app_scaffold`.
4. Call `app_register_inline` with those files and `dryRun: true`.
5. Call `app_register_inline` again with the same files and no `dryRun`.
6. Call `list_apps`.
7. Call `app_actions` for the new app id.
8. Call `invoke` only with verbs documented by `app_actions`.

`app_register_inline` writes the app bundle under `TERRANE_HOME/apps/<id>` only
when committing. It still dispatches `app.add` through core, so catalog mutation
uses the normal capability path.

The `files` argument must be a JSON array of file objects, not a JSON string.
Pass `structuredContent.files` directly. Do not JSON-stringify it.

For visible apps, pass `withUi: true` to `app_scaffold` or include a
`manifest.ui` file yourself. Keep browser code in a small `index.html` plus a
separate asset such as `ui.js` when the UI is more than a trivial button. Inline
HTML scripts are easy for small models to break.

## Choosing A Flow

Start with `workflows_list` when the user describes an outcome rather than a
tool. Pick the workflow whose summary matches the requested result:

- key/value notes app: `make_js_kv_app_no_filesystem`
- visible calendar, dashboard, form, or natural-language UI backed by app state:
  `make_js_kv_app_no_filesystem` with `app_scaffold.withUi: true`
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
constraints, errors, and examples.

## Replacing A Generated App

If registration fails because the app id already exists, first decide whether
the task is to operate the existing app or replace a failed generated app.

- To operate it, call `list_apps`, `app_actions`, then `invoke`.
- To replace it, call `capability_command` with
  `{ "name": "app.remove", "help": true }`, then use `dryRun: true`, then commit
  `app.remove` for that id, and finally rerun `app_register_inline`.

Do not remove an existing app just because registration failed; only replace
when the current task is explicitly creating a new version of that app.

When retrying `app_register_inline`, send the complete files array every time:
`manifest.json`, the backend file, every `manifest.ui` file, and referenced
assets such as `ui.js` and `style.css`. Do not send only the changed file.

## Multi-Cap App Flow

Use this when a task must prove more than a simple KV app. The scaffold kind
`js_multicap_audit` generates a backend JS app with:

- manifest resources: `kv`, `crdt`, and `relational_db`
- app actions: `seed`, `summary`, and `clearKv`
- self-described capability coverage: `app`, `kv`, `crdt`, `relational_db`,
  and `replica`

MCP-only sequence:

1. Call `workflow_info` with `{ "name": "make_js_multicap_app_no_filesystem" }`.
2. Call `app_scaffold` with `{ "kind": "js_multicap_audit", "id": "...", "name": "..." }`.
3. Register through `app_register_inline` with `dryRun: true`.
4. Commit through `app_register_inline` with the same files.
5. Call `capability_command` with `{ "name": "replica.init", "help": true }`.
6. Call `capability_command` with `{ "name": "replica.init" }`.
7. Call `capability_query` for `replica.peer` and `app.exists`.
8. Call `app_actions`, then invoke `seed`, `summary`, `clearKv`, and `summary`.

The generated app keeps `replica` outside `ctx.resource`: replica identity is an
operator/core capability checked through MCP direct tools. App code uses
`ctx.resource.kv`, `ctx.resource.crdt`, and `ctx.resource.relational_db`.

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
