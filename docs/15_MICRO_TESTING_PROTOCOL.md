# Micro Testing Protocol

## Goal

Give Codex and CI a deterministic way to test generated apps at the smallest useful level: DOM, runtime, bridge, storage, network, timers, and Zig core.

## Test file shape

Micro-tests live under:

```text
tests/micro/*.microtest.json
```

### Relationship to package `smoke-tests.json`

The platform has two test surfaces. They use a compatible step vocabulary but serve different goals.

| Aspect | `smoke-tests.json` (package-bundled) | `*.microtest.json` (platform-bundled) |
|---|---|---|
| Location | inside the webapp package | `tests/micro/` |
| Authored by | AI/app author | platform/test author or Codex |
| Runs when | at install time, before `apps.active_install_id` flips | after install, on any host, on demand |
| Driver | install validator, fake host | Codex MCP / control plane |
| Can use mocks | no — must work against real-ish bridge | yes — `network_mock_set`, `dialog_mock_set` |
| Can advance timers | no | yes — `timer_advance` |
| Can assert DB rows | no | yes — `db.assert_*` |
| Can fault-inject | no | yes — `runtime.fault_inject` |
| Failure mode | quarantines the new app version, keeps old version active | reports failure to Codex; no automatic quarantine |
| Precedence | a green smoke-tests is required to enable a version; a green microtest run is required to promote across platforms | |

A micro-test may *replay* the smoke-tests of an app as its `setup` (`runtime.run_smoke_tests`) but the converse is not allowed — smoke-tests must run on the install-time validator with no mocks.

A micro-test file is independent from the app package's `smoke-tests.json`, but may reference it.

```json
{
  "id": "notes-lite-create-note",
  "targetApps": ["notes-lite"],
  "platforms": ["macos", "linux", "windows", "android-emulator", "ios-simulator"],
  "setup": [
    { "tool": "platform.install_webapp_package", "args": { "path": "webapps/examples/notes-lite" } },
    { "tool": "platform.open_webapp", "args": { "appId": "notes-lite" } },
    { "tool": "runtime.wait_for", "args": { "kind": "idle" } }
  ],
  "steps": [
    { "tool": "runtime.assert_visible", "args": { "testId": "notes-title" } },
    { "tool": "runtime.click", "args": { "testId": "new-note-button" } },
    { "tool": "runtime.type", "args": { "testId": "note-title-input", "text": "AI generated test note" } },
    { "tool": "runtime.click", "args": { "testId": "save-note-button" } },
    { "tool": "runtime.assert_bridge_call", "args": { "method": "storage.set" } },
    { "tool": "runtime.assert_text", "args": { "text": "AI generated test note" } }
  ],
  "teardown": [
    { "tool": "platform.reset_webapp", "args": { "appId": "notes-lite", "confirm": true } }
  ]
}
```

## Selector convention

Generated apps must use stable test ids:

```html
<h1 data-testid="notes-title">Notes</h1>
<button data-testid="new-note-button">New Note</button>
```

Codex should select in this priority order:

1. `data-testid`
2. accessibility role + name
3. exact visible text
4. CSS selector as last resort

## Test operation model

Each micro-test step is a command object:

```json
{
  "tool": "runtime.click",
  "args": {
    "testId": "save-button"
  }
}
```

The runner sends the step to the MCP server. The MCP server calls the host control plane. The host returns structured result JSON.

## Required observability

After each test failure, the platform must capture:

- Screenshot.
- DOM snapshot.
- Runtime snapshot.
- Console logs.
- Bridge request/response logs.
- Permission denials.
- Storage snapshot for the app namespace.
- Core event/action log.
- Current route and focused element.

This failure bundle is what Codex uses for repair.

## Runtime idle definition

`runtime.wait_for({ kind: "idle" })` succeeds when:

- The WebView runtime has loaded.
- The app iframe is mounted.
- No pending bridge request exists.
- No pending render task exists.
- Fake timers are stable or next timer is known.
- The event/action bus is drained.

## Network mocks

Generated apps cannot call `fetch` directly. Network happens through:

```js
await AppRuntime.call("network.request", { url, method, headers, body })
```

Micro-tests can mock network calls:

```json
{
  "tool": "runtime.network_mock_set",
  "args": {
    "match": { "url": "https://api.example.test/status", "method": "GET" },
    "response": { "status": 200, "body": { "ok": true } }
  }
}
```

## Dialog mocks

File dialogs are mocked with opaque in-memory files:

```json
{
  "tool": "runtime.dialog_mock_set",
  "args": {
    "method": "dialog.openFile",
    "files": [
      { "name": "input.txt", "mime": "text/plain", "contentBase64": "SGVsbG8=" }
    ]
  }
}
```

## Timer control

Generated apps should not use raw timers for critical workflows. The runtime should provide test hooks for pending timers.

```json
{ "tool": "runtime.timer_advance", "args": { "ms": 1000 } }
```

## Core replay

Core replay verifies deterministic state-machine behavior.

```json
{
  "tool": "runtime.replay_events",
  "args": {
    "appId": "core-replay-lab",
    "events": [
      { "type": "Increment", "payload": { "by": 1 } },
      { "type": "Increment", "payload": { "by": 2 } }
    ],
    "expectFinalHash": "sha256:..."
  }
}
```

## Failure codes

Use stable error codes:

- `selector.not_found`
- `selector.not_visible`
- `text.not_found`
- `bridge.call_missing`
- `bridge.permission_denied`
- `console.error_detected`
- `storage.assertion_failed`
- `core.action_missing`
- `network.mock_missing`
- `timer.timeout`
- `platform.unavailable`
- `runtime.not_idle`
- `package.invalid`

Codex should use these codes to decide whether to patch app code, runtime code, bridge code, or platform adapter code.

## v0.3 micro-test additions

Every micro-test should start from a known package version or snapshot.

Recommended setup sequence:

```json
[
  { "tool": "platform.validate_package", "args": { "path": "webapps/examples/notes-lite" } },
  { "tool": "platform.sign_webapp_package", "args": { "path": "webapps/examples/notes-lite" } },
  { "tool": "platform.install_webapp_package", "args": { "path": "webapps/examples/notes-lite" } },
  { "tool": "platform.open_webapp", "args": { "appId": "notes-lite" } },
  { "tool": "runtime.capabilities", "args": {} },
  { "tool": "runtime.wait_for", "args": { "kind": "idle" } }
]
```

Recommended teardown sequence:

```json
[
  { "tool": "platform.create_snapshot", "args": { "type": "post-test" } },
  { "tool": "runtime.resource_usage", "args": {} },
  { "tool": "runtime.run_accessibility_audit", "args": {} },
  { "tool": "platform.reset_webapp", "args": { "appId": "notes-lite", "confirm": true } }
]
```

Micro-tests must assert no unexpected bridge calls, no console errors, and no resource budget violations unless the test is intentionally checking rejection behavior.

## Database assertions

Micro-tests may include DB-level assertions through the control plane. These assertions are dev/test-only and must use safe DB tools.

Example:

```json
{
  "type": "db.assert_app_storage",
  "appId": "notes-lite",
  "key": "notes-lite:notes",
  "expectedJsonPath": "$[0].title",
  "expected": "Hello"
}
```

Supported DB assertion families:

```text
db.assert_app_storage
db.assert_app_version_active
db.assert_bridge_call_persisted
db.assert_core_event_persisted
db.assert_test_run_persisted
db.assert_snapshot_exists
db.assert_install_report_status
```

These assertions must not be executable from generated apps.
