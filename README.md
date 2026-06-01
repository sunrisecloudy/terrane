<p align="center">
  <img src="./brand/terrane-icon.svg" alt="Terrane app icon" width="112" height="112">
</p>

<h1 align="center">Terrane</h1>

<p align="center">
  <strong>A local-first runtime for AI-generated, build-free webapps.</strong>
</p>

Terrane is a native WebView platform for apps generated as plain
`manifest.json`, `index.html`, `styles.css`, and `app.js` packages. The runtime
validates, signs, installs, runs, tests, repairs, snapshots, and rolls back those apps
without asking each generated app to become a native project or a bundled web app.

The short version: AI generates small webapps; Terrane supplies the trusted local
engine, bridge, storage, policy, tests, and native hosts.

## Use Terrane

Most people should not need to clone submodules, install native SDKs, or build
the platform before they can try Terrane.

### macOS App Download

The intended user path is a packaged macOS app from GitHub Releases:

1. Open the [latest Terrane release](https://github.com/sunrisecloudy/terrane/releases/latest).
2. Download the latest macOS artifact, for example `Terrane-macos-arm64.zip`.
3. Unzip it and open `TerraneHostMac.app`.
4. Launch one of the bundled apps, such as **Notes Lite**.

The macOS app bundle is expected to include the runtime, bundled example apps,
SQLite migrations, and Zig core library. Users should not need `git submodule`,
Zig, Swift, Rust, or Node just to open the app.

Until the first GitHub Release is published, use the local preview below.

### Local Preview

The fastest way to understand Terrane is to run the reference host, open the
runtime, and launch a bundled generated app. The reference host is the local
contract implementation that lets you use the runtime without installing every
native platform toolchain first.

Start here:

```sh
git clone https://github.com/sunrisecloudy/terrane.git
cd terrane
node --no-warnings tools/reference-host/src/server.js --port 7878
```

Then open [http://127.0.0.1:7878](http://127.0.0.1:7878).

In that first preview session:

1. Choose **Notes Lite** from the app list.
2. Create, search, edit, and delete a note.
3. Watch the **Bridge Calls** panel update as the generated app asks Terrane for
   storage, notification, and log operations.
4. Try **Task Workbench** or **Core Replay Lab** when you want to see generated
   UI sending deterministic `core.step` events through the bridge.

If you want state to survive process restarts, run the host with a SQLite file:

```sh
node --no-warnings tools/reference-host/src/server.js --port 7878 --db-file terrane-dev.sqlite
```

The mental model for the running app is:

```text
Generated app package
  -> runtime-web sandbox
  -> reference-host bridge
  -> SQLite storage, policy checks, tests, and Zig core behavior
```

Terrane is still an active implementation/spec repository, not a stable public
SDK. Use the local preview above to explore the product shape; use
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) to check what is complete,
partial, or spec-only before depending on a surface.

## Why Terrane Exists

AI is very good at producing focused app surfaces. It is much less pleasant to let
every generated app invent its own native permissions, storage model, dependency
tree, and deployment path.

Terrane keeps generated code small and constrained:

```text
AI output
  manifest.json
  index.html
  styles.css
  app.js
  smoke-tests.json
  migrations/*.json

Terrane owns
  validation
  policy audit
  canonicalization and signing
  sandboxed execution
  bridge permissions
  SQLite persistence
  snapshots and rollback
  smoke/micro tests
  native host parity
```

Generated apps use `AppRuntime.call(...)` for platform effects. They do not call
native APIs directly, do not use direct `fetch`, and do not own SQL.

## Product Shape

Terrane has two deliberate halves:

| Surface | Purpose |
|---|---|
| Open local runtime | Run generated apps locally, inspectably, and safely. |
| Private SaaS | Sync, backup, teams, marketplace publishing, enterprise governance, billing, and operations. |

The OSS server is the local Terrane engine, not the hosted SaaS backend. On
desktop it is intended to run inside the client over HTTP loopback. On mobile,
native hosts keep direct bridge dispatch because long-running embedded servers
are a poor platform fit.

See [docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md](docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md)
for the product boundary.

## Current Status

Terrane is an active implementation/spec repository. The system is not a stable
public SDK yet, but the major contract surfaces are already present:

- `runtime-web/` mounts generated apps in sandboxed frames and routes bridge calls.
- `tools/reference-host/` is the Node + SQLite reference contract implementation.
- `server/` is a Zig HTTP local engine with bridge, install, package, control, DB, snapshot, rollback, and smoke-test surfaces.
- `zig-core/` contains deterministic event-to-action core logic.
- `zig-crdt/` contains the collaborative notebook CRDT slice.
- `native/` contains iOS, macOS, Android, Windows, and Linux host targets.
- `webapps/examples/` contains five build-free example packages.
- `tests/` contains bridge, mutation, DB, micro, accessibility, security, server, CRDT, and performance fixtures.

The single source of truth for built vs planned work is
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

## Development Checks

Prerequisites for the broadest local checks:

- Node.js 22+.
- Zig 0.15.2 for Zig core/server work.
- SQLite support through the local runtime.
- Docker only for Linux native smoke tests.
- Platform SDKs only when working on a native target.

Clone submodules when you need full contributor checks, CRDT fixture generation,
or anything that touches `external-lib/loro`:

```sh
git submodule update --init --recursive
```

Run the repository contract checks:

```sh
node tools/check-repo.mjs
```

Run the reference host tests:

```sh
node --test --no-warnings tools/reference-host/test/*.test.js
```

Start the Zig local server:

```sh
cd server
zig build run-server -- --port 8088
```

Build release-style artifacts:

```sh
node --no-warnings tools/package-release.mjs --out artifacts
```

## Example Apps

Every example app is a build-free package with a manifest, HTML, CSS, JS, and
smoke tests:

| Example | What it exercises |
|---|---|
| `webapps/examples/notes-lite/` | Storage, search, toasts. |
| `webapps/examples/task-workbench/` | `core.step`, storage, stateful workflows. |
| `webapps/examples/file-transformer/` | File dialogs, core transform, save flow. |
| `webapps/examples/api-dashboard/` | Host-mediated network requests, tables, notifications. |
| `webapps/examples/core-replay-lab/` | Core replay, event log, export. |

Use these as references when creating generated apps. The package contract lives
in [docs/04_WEBAPP_PACKAGE_SPEC.md](docs/04_WEBAPP_PACKAGE_SPEC.md).

## Generated App Rules

Generated app packages must stay simple and portable:

- use HTML, CSS, and vanilla JavaScript only;
- include `manifest.json`, `index.html`, `styles.css`, and `app.js`;
- include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy`;
- use stable `data-testid` attributes for interactive elements;
- call platform effects through `AppRuntime.call(...)`;
- use `storage.*` bridge methods instead of `localStorage`, IndexedDB, cookies, or SQL;
- use `network.request` instead of direct `fetch`;
- never send `appId` in request bodies because Terrane derives it from the mount channel.

Do not add React, TypeScript, JSX, Vite, Next.js, npm dependencies, or a build
step to a generated app package unless a future runtime capability explicitly
adds that support.

## Architecture

```text
Generated app package
  HTML/CSS/vanilla JS inside sandboxed iframe
        |
        v
runtime-web
  AppRuntime.call, mount channels, permissions, budgets
        |
        v
Host bridge
  native bridge or Zig local server
        |
        v
Platform services
  SQLite storage, dialogs, notifications, network policy, logs
        |
        v
Zig core
  deterministic event -> action state machines
```

The reference host is the oracle. Native hosts and the server are expected to
match its bridge responses for the same fixtures, after stripping fields that
are explicitly non-deterministic.

## Repository Map

| Path | What lives there |
|---|---|
| `docs/` | Normative product, runtime, security, package, DB, testing, and release specs. |
| `runtime-web/` | Browser/WebView runtime that mounts generated apps. |
| `server/` | Zig local server and HTTP bridge/control surface. |
| `tools/reference-host/` | Reference contract host used by tests and Codex workflows. |
| `tools/codex-platform-mcp/` | Codex MCP bridge to the platform control plane. |
| `zig-core/` | Deterministic core state machine and C ABI. |
| `zig-crdt/` | Notebook CRDT package and C ABI. |
| `native/` | iOS, macOS, Android, Windows, and Linux hosts. |
| `webapps/examples/` | Canonical generated app packages. |
| `tests/` | Fixtures, security mutations, micro-tests, DB tests, smoke tests, and performance checks. |
| `db/` | SQLite migrations and Postgres-compatible logical schema. |
| `brand/` | Terrane visual assets used by public docs. |

## Key Docs

| Need | Read |
|---|---|
| Product baseline | [docs/00_PRD.md](docs/00_PRD.md) |
| Architecture | [docs/01_ARCHITECTURE.md](docs/01_ARCHITECTURE.md) |
| Runtime bridge | [docs/03_RUNTIME_API_SPEC.md](docs/03_RUNTIME_API_SPEC.md) |
| Generated app format | [docs/04_WEBAPP_PACKAGE_SPEC.md](docs/04_WEBAPP_PACKAGE_SPEC.md) |
| Native/platform rules | [docs/05_NATIVE_PLATFORM_REQUIREMENTS.md](docs/05_NATIVE_PLATFORM_REQUIREMENTS.md) |
| Security model | [docs/07_SECURITY_MODEL.md](docs/07_SECURITY_MODEL.md) |
| Test plan | [docs/08_TEST_PLAN.md](docs/08_TEST_PLAN.md) |
| Codex control plane | [docs/14_CODEX_CONTROL_PLUGIN.md](docs/14_CODEX_CONTROL_PLUGIN.md) |
| Repair loop | [docs/25_CODEX_REPAIR_LOOP.md](docs/25_CODEX_REPAIR_LOOP.md) |
| Database schema | [docs/27_DATABASE_SCHEMA.md](docs/27_DATABASE_SCHEMA.md) |
| Reference host | [docs/32_REFERENCE_HOST_SPEC.md](docs/32_REFERENCE_HOST_SPEC.md) |
| Local/SaaS split | [docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md](docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md) |

## Working On Terrane

Specs are normative; README files are orientation. When behavior is unclear,
prefer the relevant document under `docs/`, then check
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for current implementation
state.

Recommended habits:

- keep generated apps build-free;
- add or update fixtures when behavior changes;
- run reference-host tests before native parity work;
- run target-specific native smoke tests when touching native bridges;
- keep SaaS-only auth, billing, admin, and operations code outside this OSS local runtime.

## License

MIT. See [LICENSE](LICENSE).
