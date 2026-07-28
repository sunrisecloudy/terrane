---
name: terrane-new-app
description: Create, install, verify, or update a Terrane app through the current GUI-owned MCP builder and native host. Use when asked to create, make, scaffold, add, install, run, or visually verify a Terrane app; modify an app backend or UI; add localization, smoke tests, resources, interop, file intake, or a meaningful app-owned sidebar; or promote a GUI-tested app into the repository. Not for adding a new command namespace or ctx.resource surface; those are capabilities and follow docs/cap-best-practice/.
---

# Create a Terrane app

Build through the live Terrane contract, not from memory. A source bundle,
preview, backend invocation, or passing unit test alone does not prove a visible
app is complete.

## Establish the current contract

Before writing:

1. Inspect checkout and worktree state. Identify the worktree that owns local
   `main`; do not assume the current checkout is current or clean.
2. Read `docs/APP_API.md` and `host/mcp/docs/APP_BUILDING.md`.
3. Inspect `terrane_api::mcp_tools()` or call `tools/list` when tool shape
   matters. Query capability docs for the resources the app will use; do not
   copy a remembered namespace list.
4. Use this skill's `assets/app-template/` as a starter/reference, but prefer
   live tool-generated scaffold contracts when building through MCP.

If this skill or its template disagrees with the current validator or generated
contract, the current source wins. Update the skill/template in the same bounded
change when appropriate so the next app does not repeat the drift.

## Decide app or capability

- **App**: a bundle that composes existing `ctx.resource.*` namespaces behind
  user-facing actions. Use this skill.
- **Capability**: a new command namespace, runtime surface, or
  `ctx.resource.*` API. Follow `docs/cap-best-practice/README.md`.

## Use the GUI-owned MCP path

For a real new app, connect `terrane-mcp` to the running native GUI Core. GUI
and MCP must resolve the same `TERRANE_HOME`. Prefer GUI-only launch-or-attach
mode when the client must never open a second Core. The authenticated loopback
discovery record is host-owned; never copy or expose its per-launch token.

The built-in App Builder may use Codex, Claude Code, or OpenCode to generate
files and render an in-memory preview. Preview does **not** install the bundle,
write it under `TERRANE_HOME/apps`, or add it to the catalog. Continue through
the staged MCP flow.

### Canonical staged flow

1. For blank context, call `workflows_list`, then the matching
   `workflow_info`. Use `app_recipe` when scaffold/resource orientation helps.
2. Start a visible app with:

   ```json
   {
     "id": "my-app",
     "name": "My App",
     "kind": "js_kv_app",
     "withUi": true
   }
   ```

   Call `app_build_start` with those arguments. Use `withUi: true` for anything
   a person sees. Do not choose the backend-only notes demo for a UI request.
3. Replace changed files using `app_build_put_file`. Send complete file
   contents. Batch writes are allowed and all-or-nothing.
4. Call `app_build_validate`. Fix every error and consciously review warnings.
5. Call `app_build_commit` with the returned `draftId` and
   `validationToken`. Commit is create-only and deletes the draft.
6. Confirm with `list_apps`, discover verbs with `app_actions`, then call
   `invoke` using a documented verb.

If work stalls, recover with `app_build_list`, then `app_build_get`. Do not
start duplicate drafts or re-read unchanged scaffold files without a reason.
The older `app_scaffold`/`app_register_inline` and filesystem bundle flows are
compatibility paths, not the default for a new app.

## Bundle contract

### Manifest

Use the current manifest schema. A typical visible JS app includes:

```json
{
  "id": "my-app",
  "name": "My App",
  "version": "0.1.0",
  "runtime": "js",
  "backend": "main.js",
  "ui": "index.html",
  "icon": "icon.svg",
  "resources": ["kv"],
  "interfaces": ["items"],
  "sidebar": {
    "mode": "none",
    "reason": "This app has one workspace and no useful lower-sidebar navigation."
  }
}
```

- Keep `id` stable and kebab-case.
- `ui` is a string path, not an object. Omit it for backend-only apps.
- `interfaces` includes `items`; `inbox` is implied.
- Request only resources the backend actually uses. A request is not a grant.
- Checked-in apps declare an SVG icon. User-installed apps may temporarily use
  the host fallback, but a finished first-party app should not.
- Add `fileTypes` only for intentional `common.receive("blob", ref)` delivery.

Every new app makes an explicit `sidebar` decision:

- Use `{"mode":"section"}` only for meaningful, durable app-owned workspaces or
  items. Publish them with
  `setSidebarSection({title,items,selectedItemId,createLabel?})`, handle
  `onSidebarItemSelect`, and handle `onSidebarCreate` when creation is useful.
- Otherwise use `{"mode":"none","reason":"..."}` with an app-specific reason.
- The app-owned lower section is not the host's Apps list. Never add fake or
  decorative rows merely to satisfy validation.

### Backend

Prefer an `actions` table so action metadata, dispatch, usage, and
`__actions__` discovery share one source of truth.

- A run starts in a fresh QuickJS context. Persist only through resources.
- Every action returns a string; JSON-stringify structured values.
- Avoid imports, modules, `require`, Node/Deno APIs, unbounded loops, ambient
  filesystem state, clocks, and randomness outside a declared resource.
- Feature-detect every `ctx.resource.<namespace>`. Resources are default-deny
  and absent until granted.
- Wrap optional `kv.get`/`rm` operations because missing keys can throw.
- Prefer one KV key per fact and stable string item IDs; parse stored values
  defensively.

Required common verbs:

| Verb | Contract |
| --- | --- |
| `common.receive` | `(kind, payloadJson)` receives links, shares, or blob references |
| `common.list` | `(filterJson?)` returns a JSON array of `{id,title,kind}` |
| `common.get` | `(id)` returns item JSON or typed `NotFound` JSON |

Actions-table defaults are acceptable only when the app has no meaningful item
behavior. Override `common.list` and `common.get` for real items. For cross-app
handoff, use `ctx.resource.interop.send` and the host powerbox picker; do not
hardcode another app.

### UI

The UI calls only its own backend:

```js
await window.terrane.invoke("verb", "arg1", "arg2");
```

Pass positional string arguments, not an array or object. Keep backend behavior
in `main.js` and browser behavior in the UI. Use `textContent`, never
`innerHTML`, for app/backend/user text.

For localization:

- Keep `i18n/en.json` complete as fallback and key inventory.
- Prefer all supported locale catalogs for a finished user-facing app.
- Always supply `default:` to `window.terrane.t`.
- Re-render on `onMessages`, set `documentElement.dir`, and use CSS logical
  properties for RTL.
- Respect host theme and document-name APIs when relevant.
- Keep a portable fallback for host-specific helpers such as the macOS picker.

### Smoke tests

Include `tests.json` for meaningful backend behavior. Validation runs tests
in an isolated runtime with temporary grants for the resources declared by the
manifest. Test action discovery, real resource-backed behavior, `common.list`,
and typed `common.get` failure. Test graceful ungranted behavior separately
through the real permission handshake. Use `contains`, `jsonSubset`, or `shape`
assertions. A missing `tests.json` is allowed but is not best practice for a
non-trivial app.

## Permissions and runtime verification

`manifest.resources` requests namespaces; it never grants them. A first
`app_actions` or `invoke` may return `permission_required`.

1. Treat that response as the expected handshake, not an app failure.
2. Surface the `grantCommands` or `adminUrl`. MCP cannot grant itself.
3. Let the trusted GUI/admin/CLI approve, poll `permission_check` when needed,
   then retry the exact same invocation.
4. Do not delete, rewrite, or re-register the app because permission is pending.

Verify at least one real mutation and readback. Check app logs when relevant and
run replay verification for persistent behavior.

## Native acceptance

For a visible app, backend and preview checks are insufficient. Test in the
real native Terrane GUI:

1. App appears in live discovery and opens from the host.
2. HTML and browser JS load without errors.
3. A real control calls the intended backend verb.
4. The displayed result matches the request.
5. State survives navigation/reload or relaunch as appropriate.
6. Permission UI works on first use.
7. Sidebar selection/create persists correctly, or the explicit opt-out is
   accurate.
8. Theme, localization, empty/error states, keyboard interaction, and narrow
   layout are usable for the requested surface.

Do not claim GUI acceptance from generated files, a preview iframe, screenshots
without interaction, source inspection, or backend invokes alone.

## Promote a checked-in app

When the app must become a first-party bundle under `apps/<id>`:

1. Promote the exact GUI-tested bundle; verify byte equivalence before changing
   it further.
2. Run `app_bundle_validate` or the equivalent real install validation.
3. Rebuild committed frontend output after source edits.
4. Run focused app tests plus native packaging tests that prove every checked-in
   bundle and icon ships.
5. Use the repository-required shared Cargo cache:

   ```sh
   scripts/with-cargo-cache.sh cargo test --workspace --locked
   scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
   ```

6. Preserve unrelated dirty work. Stage only the app/skill files in scope.
   Commit, merge, push, or deploy only when explicitly requested.

## Done checklist

- [ ] Current `APP_API.md`, MCP tool schema, and capability docs were consulted
- [ ] GUI and MCP used the same `TERRANE_HOME`
- [ ] Staged builder validated and committed with a matching token
- [ ] Manifest requests only used resources and makes an explicit sidebar decision
- [ ] Actions return strings; required common verbs match real item behavior
- [ ] Optional resource reads degrade safely before grants
- [ ] UI uses positional `window.terrane.invoke`, safe text, theme, and localization
- [ ] `tests.json` passes through real bundle validation
- [ ] Permission handshake, mutation/readback, and replay were verified
- [ ] A real native GUI interaction passed
- [ ] Checked-in bundle/icon/frontend packaging passed when promoting to source
- [ ] Unrelated work was preserved; no unrequested merge, push, or deployment
