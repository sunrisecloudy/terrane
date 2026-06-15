# Contributing To Terrane

Specs are normative; README files are orientation. When behavior is unclear,
prefer the relevant document under `docs/`, then check
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for current implementation
state.

## Development Setup

Prerequisites for the broadest local checks:

- Node.js 22+.
- Rust/Cargo for Forge core, FFI, server, and native package work.
- SQLite support through the local runtime.
- Docker only for Linux native smoke tests.
- Platform SDKs only when working on a native target.

Clone submodules only when you need full contributor checks, CRDT fixture
generation, or anything that touches `external-lib/loro`:

```sh
git submodule update --init --recursive
```

The `external-lib/loro` submodule is a pinned CRDT oracle for notebook fixtures.
It is not needed for people who only download and run the macOS app.

## Development Checks

Run the repository contract checks:

```sh
node tools/check-repo.mjs
```

Run the reference host tests:

```sh
node --test --no-warnings tools/reference-host/test/*.test.js
```

Start the Forge local server:

```sh
cd forge
cargo run -p forge-server -- --bind 127.0.0.1:8787
```

Build static release-style artifacts:

```sh
node --no-warnings tools/package-release.mjs --out artifacts
```

Build the macOS native app and release disk image on macOS:

```sh
node --no-warnings tools/package-release.mjs --out artifacts --build-native-macos
```

That writes both the inspectable app bundle and the user-downloadable disk
image:

```text
artifacts/native-apps/macos/<target>/terrane.app
artifacts/native-apps/macos/<target>/Terrane-<target>.dmg
```

Attach the `.dmg` to GitHub Releases for normal users. The `.app` bundle is
kept in artifacts for CI inspection, smoke tests, and debugging.

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

## Repository Map

| Path | What lives there |
|---|---|
| `docs/` | Normative product, runtime, security, package, DB, testing, and release specs. |
| `forge/` | Rust v1 core, storage, sync/CRDT, FFI, CLI, and server workspace. |
| `runtime-web/` | Browser/WebView runtime that mounts generated apps. |
| `tools/reference-host/` | Reference contract host used by tests and Codex workflows. |
| `tools/codex-platform-mcp/` | Codex MCP bridge to the platform control plane. |
| `native/` | iOS, macOS, Android, Windows, and Linux hosts. |
| `webapps/examples/` | Canonical generated app packages. |
| `tests/` | Fixtures, security mutations, micro-tests, DB tests, smoke tests, and performance checks. |
| `db/` | SQLite migrations and Postgres-compatible logical schema. |
| `brand/` | Terrane visual assets used by public docs. |

## Key Docs

| Need | Read |
|---|---|
| Product baseline | [prd-merged/00-master-prd.md](prd-merged/00-master-prd.md) |
| Architecture | [prd-merged/01-core-runtime-prd.md](prd-merged/01-core-runtime-prd.md) |
| Runtime bridge | [docs/03_RUNTIME_API_SPEC.md](docs/03_RUNTIME_API_SPEC.md) |
| Generated app format | [docs/04_WEBAPP_PACKAGE_SPEC.md](docs/04_WEBAPP_PACKAGE_SPEC.md) |
| Native/platform rules | [prd-merged/01-core-runtime-prd.md](prd-merged/01-core-runtime-prd.md), [forge/spec/](forge/spec/) |
| Security model | [docs/07_SECURITY_MODEL.md](docs/07_SECURITY_MODEL.md) |
| Test plan | [docs/08_TEST_PLAN.md](docs/08_TEST_PLAN.md) |
| Codex control plane | [docs/14_CODEX_CONTROL_PLUGIN.md](docs/14_CODEX_CONTROL_PLUGIN.md) |
| Repair loop | [docs/25_CODEX_REPAIR_LOOP.md](docs/25_CODEX_REPAIR_LOOP.md) |
| Database schema | [docs/27_DATABASE_SCHEMA.md](docs/27_DATABASE_SCHEMA.md) |
| Reference host | [tools/reference-host/](tools/reference-host/), [docs/35_PUBLIC_CONTRACT_EXPORT.md](docs/35_PUBLIC_CONTRACT_EXPORT.md) |
| Local/SaaS split | [docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md](docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md) |

## Working Habits

- Keep generated apps build-free.
- Add or update fixtures when behavior changes.
- Run reference-host tests before native parity work.
- Run target-specific native smoke tests when touching native bridges.
- Keep SaaS-only auth, billing, admin, and operations code outside this OSS local runtime.
