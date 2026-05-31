# Example App API Coverage

This document is the normative coverage map for the five bundled generated apps required by `docs/00_PRD.md` G6. The apps are fixtures, not demos outside the contract: when a bridge method, manifest capability, smoke-test behavior, or policy invariant changes, this file must be updated in the same change.

## Coverage Invariants

- The bundled set is exactly `notes-lite`, `task-workbench`, `file-transformer`, `api-dashboard`, and `core-replay-lab`.
- Each app is build-free HTML/CSS/vanilla JavaScript and ships `manifest.json`, `index.html`, `styles.css`, `app.js`, and `smoke-tests.json`.
- Each app calls platform features only through `AppRuntime.call(method, params)`.
- Each app declares every bridge permission it uses in `manifest.permissions` and `manifest.capabilities`.
- Each app includes `dataVersion`, `resourceBudget`, `networkPolicy`, `trust`, `compatibility`, and App Store `contentRating`.
- Each app has stable `data-testid` selectors for interactive controls; smoke tests must use `data-testid` selectors or DOM text, and user-facing interaction targets must stay testable.

## App Matrix

| App | Path | Primary contract | Required capabilities | Optional capabilities | Smoke coverage |
|---|---|---|---|---|---|
| Notes Lite | `webapps/examples/notes-lite` | CRUD notes through storage with toast/log feedback | `storage.read`, `storage.write` | `notification.toast`, `app.log` | Empty-state render, create note, `storage.set`, toast |
| Task Workbench | `webapps/examples/task-workbench` | Stateful task workflow backed by Zig `core.step` actions with bounded large-list rendering | `core.step`, `storage.read`, `storage.write` | `notification.toast`, `app.log` | Add task, `core.step`, `storage.set`, toast |
| File Transformer | `webapps/examples/file-transformer` | Native file open/save plus deterministic text transform | `core.step`, `dialog.openFile`, `dialog.saveFile`, `storage.read`, `storage.write` | `notification.toast`, `app.log` | Open, transform, save, `core.step`, dialog calls, `storage.set` |
| API Dashboard | `webapps/examples/api-dashboard` | Manifest-gated network request and saved request history | `network.request`, `storage.read`, `storage.write` | `notification.toast`, `app.log` | Send request, `network.request`, `storage.set`, toast |
| Core Replay Lab | `webapps/examples/core-replay-lab` | Core event replay log and fixture export | `core.step`, `storage.read`, `storage.write`, `dialog.saveFile` | `notification.toast`, `app.log` | Send event, export, `core.step`, `storage.set`, `dialog.saveFile`, toast |

## Bridge Method Matrix

| Bridge method | Covered by | Required evidence |
|---|---|---|
| `storage.get` | Notes Lite load, Task Workbench load, API Dashboard load, Core Replay Lab load | Source calls plus example load/smoke tests |
| `storage.set` | Notes Lite save, Task Workbench persist, File Transformer transform result, API Dashboard history, Core Replay Lab event log | Smoke tests assert this call for every app that mutates state |
| `storage.remove` | Notes Lite clear-all path | Source fixture and package validation; targeted bridge fixture covers the response contract |
| `storage.list` | Bridge fixture suite | Not required in a reference UI flow, but must stay covered by `tests/fixtures/bridge/valid-storage-list.json` |
| `core.step` | Task Workbench, File Transformer, Core Replay Lab | Smoke tests assert `core.step`; Zig replay/unit tests verify deterministic core behavior |
| `dialog.openFile` | File Transformer | Smoke test asserts the bridge call; micro-tests/golden flows provide dialog mocks |
| `dialog.saveFile` | File Transformer, Core Replay Lab | Smoke tests assert the bridge calls; micro-tests/golden flows provide dialog mocks |
| `notification.toast` | All five apps | Smoke tests assert toast on state-changing paths |
| `network.request` | API Dashboard | Smoke test asserts the bridge call against manifest-allowed source code; micro-tests/golden flows provide network mocks |
| `app.log` | Notes Lite and optional support in all manifests | Source fixture plus bridge contract; not every smoke path must assert a log line |
| `runtime.capabilities` | Runtime/host contract, not generated app UI | Covered by runtime capability fixtures and host bridge tests |

## Policy Coverage

| Policy surface | Bundled app expectation | Verification |
|---|---|---|
| Build-free package | No build files, package managers, TypeScript, JSX, React, or bundlers in app packages | `tools/check-repo.mjs` and package validator tests |
| Direct network ban | Apps must not call `fetch`, `XMLHttpRequest`, WebSocket, EventSource, or remote scripts | Package validator and mutation/security fixtures |
| Storage API ban | Apps must not use `localStorage`, `sessionStorage`, IndexedDB, cookies, or SQL | Package validator and mutation/security fixtures |
| Network policy | Only API Dashboard has an allowlist entry, and only through `networkPolicy` | Manifest schema, package validator, network-policy tests |
| App identity | Apps never set `appId` in bridge params; runtime derives it from the mount context | Runtime tests reject app-supplied `appId` params |
| Content rating | All bundled manifests include App Store `4+` ratings | Manifest schema, reference-host validator, iOS bundled-index tests |
| Accessibility | Interactive controls use stable selectors and accessible names/labels | Accessibility microtests and reference-host accessibility audit |

## Test Evidence

The reference coverage is exercised by:

- `node --no-warnings tools/check-repo.mjs` for package shape, canonical examples, static policy, and manifest sanity.
- `node --test --no-warnings tools/reference-host/test/example-load-acceptance.test.js` for validate/install/open/snapshot/smoke coverage across all examples.
- `node --test --no-warnings tools/reference-host/test/test-runner.test.js` for checked-in `smoke-tests.json` and `tests/micro` execution.
- `node --test --no-warnings tools/reference-host/test/package-validator.test.js` for manifest and generated-source policy failures.
- `node --test --no-warnings tools/reference-host/test/security-fixtures.test.js` for runtime-denied malicious packages.
- `node --test --no-warnings tools/reference-host/test/server-bridge-contract.test.js` for server parity with the bridge fixtures.

Adding or removing an example app, permission, bridge method, smoke-test expectation, or manifest policy field must update this document and the relevant tests before the change is considered verified.
