# Forge Example Applets

The Forge v1 example applets live under `forge/examples/`. They are TypeScript
applets that install through `applet.install`, run through `runtime.run`, render
Forge UI trees, and replay through `runtime.replay`.

These examples are the replacement set for the legacy build-free
`webapps/examples/` packages. Packaging and runtime consumers may still point at
the legacy tree during the cutover, but the Forge examples are now executable
through the core command facade.

| Example | Public API coverage | Primary test |
| --- | --- | --- |
| `notes-lite` | `ctx.db`, `ctx.ui`, `ctx.time` | `cargo test -p forge-cli --test forge_examples notes-lite` |
| `task-workbench` | `ctx.db`, `ctx.storage`, structured `ctx.db.query` | `cargo test -p forge-cli --test forge_examples task-workbench` |
| `file-transformer` | `ctx.files`, `ctx.db`, `ctx.ui` | `cargo test -p forge-cli --test forge_examples file-transformer` |
| `api-dashboard` | `ctx.net`, `ctx.db`, `ctx.ui` | `cargo test -p forge-cli --test forge_examples api-dashboard` |
| `core-replay-lab` | `ctx.time`, `ctx.random`, `ctx.storage`, `ctx.db` | `cargo test -p forge-cli --test forge_examples core-replay-lab` |
| `calendar-planner` | `ctx.db`, `ctx.ui`, agenda storage | `cargo test -p forge-cli --test forge_examples calendar-planner` |

## Executable gates

Library path (all six apps in one test):

```sh
cd forge
cargo test -p forge-cli --test forge_examples --locked
```

CLI subprocess path:

```sh
cd forge
cargo test -p forge-cli --test bundled_apps_cli_e2e --locked
```

M0a spine (notes-lite only):

```sh
cd forge
cargo run -p forge-cli -- demo
cargo test -p forge-cli --test e2e --locked
```

Legacy reference-host smoke (build-free `webapps/examples/*`):

```sh
node --test --no-warnings tools/reference-host/test/example-load-acceptance.test.js
```

## HTML reference

The generated public API page links every example with source snippets and test commands:

```sh
node --no-warnings tools/build-forge-api-docs.mjs
```

Open `forge/docs/public-api/index.html` locally, or `GET /docs` on `forge-server`.