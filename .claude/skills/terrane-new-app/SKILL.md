---
name: terrane-new-app
description: Best practice and step-by-step workflow for creating a new Terrane app — a JS bundle under apps/ with manifest.json, a main.js backend over ctx.resource.*, an optional index.html UI over window.terrane, i18n catalogs, and tests.json smoke tests. Use when asked to create, make, scaffold, or add a new Terrane app, write or fix an app backend or UI, wire app localization, or install, run, grant, and verify an app bundle. Not for new commands/namespaces — those are capabilities (Rust crates) and follow docs/cap-best-practice/.
---

# Creating a new Terrane app

## App or capability? Decide first

- **App** — a JS bundle in `apps/<id>/` that composes existing resources
  (`ctx.resource.kv`, `crdt`, `local-model`, …) behind user-facing verbs. This
  skill.
- **Capability** — a new command namespace / new `ctx.resource.*` surface. That
  is a Rust crate under `rust/crates/terrane-cap-<name>/`; follow
  `docs/cap-best-practice/README.md` instead.

## Authoritative sources (read before writing code)

- `docs/APP_API.md` — the full app contract, **drift-guarded by tests** (the
  `ctx.resource` reference is generated; regenerate with `UPDATE_DOCS=1 cargo
  test`). Trust it over memory. Key sections: "Recommended: an `actions`
  table", "Required common verbs and items", "Default-deny resources & the
  permission handshake", "Client (UI)", "Localization", "Manifest".
- Example apps: `apps/todo` (plain JS + kv, the canonical shape),
  `apps/bmi-calculator` (frontend build step, `src/` → `dist/`),
  `apps/chat` (kv + local-model).
- Template to copy: this skill's `assets/app-template/` — a complete minimal
  bundle (manifest, actions-table backend, UI with localize, i18n, tests.json,
  icon).

## The contract (non-negotiable)

- The backend is invoked as `handle([verb, ...args])` and **must return a
  string**. JSON-stringify structured replies.
- Each run is a **fresh context** — no state survives between runs. All
  persistence goes through resources; writes are recorded as events and replay
  **without re-running your JS** (Option A). Never use the clock, randomness,
  or external state except through a resource.
- Resources are **default-deny**. `manifest.resources` only *requests* a
  namespace; `ctx.resource.<ns>` is **absent** until an admin grants it.
  Feature-detect every namespace and degrade with a plain string — never throw.
- The UI's only bridge is `window.terrane.invoke(verb, ...args)` to its **own**
  backend. It never touches `ctx.resource` or names another app.
- A backend run has a wall-clock budget — no unbounded loops.

## Workflow

### 1. Scaffold

Copy `assets/app-template/` from this skill to `apps/<id>/` and rename: set
`id` (kebab-case, stable forever), `name`, and the strings in `main.js` /
`index.html`. Or copy `apps/todo` for a working reference.

### 2. Manifest

```json
{
  "id": "my-app", "name": "My App", "version": "0.1.0",
  "runtime": "js", "backend": "main.js", "ui": "index.html",
  "icon": "icon.svg", "resources": ["kv"], "interfaces": ["items"]
}
```

- `interfaces`: `items` is required, `inbox` is implied. Omit `ui` for
  CLI-only apps. `fileTypes: [{ext,mime}]` opts into `terrane open <file>`
  delivery via `common.receive("blob", ref)`.
- Request only the resources actually used. Grantable today: `kv`, `crdt`,
  `relational_db`, `build` (others are skipped by the granter, not blocked —
  see APP_API.md "Grantable namespaces & verbs").

### 3. Backend (`main.js`)

Use the **actions table** (not a hand-rolled `handle`): each entry holds
summary + args + `run(args, usage)` together, and the runtime synthesizes verb
dispatch, `__actions__` discovery (the MCP `app_actions` tool), `usage()`, and
unknown-verb help.

Required common verbs — the runtime scaffolds defaults for actions-table apps;
**override `common.list` / `common.get` whenever the app has real items**
(the template shows how):

| Verb | Contract |
| --- | --- |
| `common.receive` | `(kind, payloadJson)` — deep links, file imports, share deliveries enter here |
| `common.list` | `(filterJson?)` → JSON array of `{id,title,kind}`; `[]` is valid |
| `common.get` | `(id)` → item JSON or `{"ok":false,"error":{"code":"NotFound","id":"…"}}` |

Best practice, paid for in review:

- **One kv key per fact** — each mutation is exactly one recorded `kv.*` event
  (e.g. `seq` + `item:<id>`), so replay folds cleanly. Don't serialize one big
  JSON blob per app.
- Parse stored values defensively (`parseInt` + `isNaN` guard); a key can be
  missing or stale.
- Item ids are **stable strings**; items are addressable as
  `terrane://app/<id>/item/<itemId>` and resolve through live `common.get`.
- To hand data to another app, call `ctx.resource.interop.send(interface,
  kind, payloadJson)` — the host raises the powerbox picker; never hardcode a
  target app.

### 4. UI (`index.html`)

- Render backend JSON from an `items`-style verb; assign text with
  `textContent`, **never `innerHTML`**.
- Localize: set `document.documentElement.dir = window.terrane.getDir()`, use
  `window.terrane.t(key, { default: "…" })` on `[data-i18n]` elements, and
  re-run in `onMessages` (template has the snippet). Always pass `default:` so
  the first paint works before the bundle arrives and headless hosts keep
  working.
- Use CSS logical properties (`margin-inline-start`, `text-align: start`) so
  RTL (`ar`) mirrors. Respect the host theme (`color-scheme: light dark`;
  `window.terrane.getTheme()`/`onTheme` if the app needs to react).
- Optional top-bar document name: `getDocument`/`setDocument`/`onDocument`.

### 5. i18n catalogs

Ship flat JSON per language at `apps/<id>/i18n/<code>.json`. Keep **`en`
complete** — it is the fallback and the key inventory. Supported codes:
`en, es, zh-Hans, ar, pt-BR, fr, de, ja, id, th-TH, ko, vi`. Hosts seed
catalogs into public KV on startup; `terrane i18n import <path>` does it on
demand. Backend return strings are not auto-localized (v1) — localize the UI.

### 6. Optional: frontend build step

For a TSX/bundled UI (see `apps/bmi-calculator`): add to the manifest

```json
"ui": "dist/index.html",
"frontend": { "tool": "terrane-app-build", "entry": "src/main.tsx", "styles": ["src/app.css"] }
```

then rebuild `dist/` with `terrane app build apps/<id>` after every `src/`
edit. Commit `dist/` (bundles ship prebuilt); add the app-local `.gitignore`
with `!dist/` + `!dist/**` to un-ignore it.

### 7. Smoke tests (`tests.json`)

Add backend smoke cases beside the manifest; they run during bundle validation
(`app install`, `app.import`, builder staging). Expectations: `contains`
(substring), `jsonSubset`, `shape` (type names: `string`, `number`, `boolean`,
`null`, `array`, `object`, `any`). Write cases that pass **without grants**
(validation runs ungranted — e.g. `items` → `[]`, `common.get` unknown id →
NotFound JSON).

### 8. Install, grant, run, verify

```sh
# Dev catalog (repo apps). TERRANE_HOME defaults to ./.terrane
cargo run -p terrane-host --bin terrane -- app add <id> <Name…> --source apps/<id>
# or validate the bundle like a real install (runs tests.json):
cargo run -p terrane-host --bin terrane -- app install apps/<id>

# Resources are default-deny — grant before first use (local subject is fixed):
cargo run -p terrane-host --bin terrane -- auth grant user:local-owner <id> kv

# Exercise the backend and verify:
cargo run -p terrane-host --bin terrane -- run <id> <verb> [args…]
cargo run -p terrane-host --bin terrane -- logs <id>        # backend log buffer
cargo run -p terrane-host --bin terrane -- replay           # replay-identity check
```

If an invoke returns a `permission_required` error instead, that is the
handshake, not a bug: run the `grantCommands` it contains (or approve at its
`adminUrl`), then retry the same call — see APP_API.md "Default-deny resources
& the permission handshake".

To see the UI, start the web host (`.claude/launch.json` → `terrane-web`,
http://127.0.0.1:8795) and open the app from the shell. In a fresh agent
worktree, first copy the canonical home:
`scripts/copy-terrane-home.sh --to "$PWD/.terrane"`.

### 9. Done checklist

- [ ] `manifest.json` valid; `interfaces` includes `items`; only-needed `resources`
- [ ] actions table; every `run` returns a string; `common.list`/`common.get` real
- [ ] every `ctx.resource.<ns>` feature-detected; degrades without grants
- [ ] one kv key per fact; no clock/randomness outside resources
- [ ] UI uses only `window.terrane`; `textContent`; localize + `dir`; logical CSS
- [ ] `i18n/en.json` complete (+ the other 11 codes when localizing)
- [ ] `tests.json` passes ungranted: `terrane app install apps/<id>` green
- [ ] `terrane run <id> …` works end-to-end after `auth grant`; `terrane replay` green
- [ ] stage only your own files; commit small and green
