# Bundled Example Apps

Canonical bundled apps live under `webapps/examples/`. They are build-free HTML/CSS/JS
packages validated and installed by the reference host and native shells, launched from
`runtime-web`, and listed in `forge/data/bundled-apps.json`.

| Example | Bridge / runtime coverage | Primary test |
| --- | --- | --- |
| `notes-lite` | `storage.*`, `notification.toast`, `app.log` | `tests/micro/notes-lite-create-note.microtest.json` |
| `task-workbench` | `core.step`, `storage.*` | `tests/micro/task-workbench-core-storage.microtest.json` |
| `file-transformer` | `dialog.*`, `storage.*`, `core.step` | `tests/micro/file-transformer-core-storage.microtest.json` |
| `api-dashboard` | `network.request`, `storage.*` | `tests/micro/api-dashboard-network.microtest.json` |
| `core-replay-lab` | `core.step`, `storage.*` | `tests/micro/core-replay-lab-core-storage.microtest.json` |
| `calendar-planner` | `core.step`, `storage.*` | `tests/micro/calendar-planner-core-storage.microtest.json` |
| `test-camera` | `resource.invoke`, `resource.read`, `resource.materialize`, `storage.*` | `webapps/examples/test-camera/smoke-tests.json` |

## Executable gates

Reference-host acceptance (every bundled example):

```sh
node --test --no-warnings tools/reference-host/test/example-load-acceptance.test.js
```

Package validation:

```sh
node --test --no-warnings tools/reference-host/test/package-validator.test.js
```

Repo parity (`bundled-apps.json` ↔ `webapps/examples/`):

```sh
node --no-warnings tools/check-repo.mjs
```

M0a Forge spine (internal demo applet only — not a bundled webapp):

```sh
cd forge
cargo run -p forge-cli -- demo
cargo test -p forge-cli --locked
```

## HTML reference

```sh
node --no-warnings tools/build-forge-api-docs.mjs
```

Open `forge/docs/public-api/index.html` locally, or `GET /docs` on `forge-server`.