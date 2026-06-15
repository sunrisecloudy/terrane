# Architecture

> **⚠️ SUPERSEDED (2026-06-12):** This document describes the v0.4 legacy WebView/Zig prototype. The normative v1 architecture is `prd-merged/` plus the Rust Forge workspace under `forge/`.

## 1. System layers

```text
┌──────────────────────────────────────────────────────────┐
│ Native platform shell                                     │
│ iOS/macOS Swift, Android Kotlin, Windows C++/WinRT,       │
│ Linux GTK/C, Zig server                                   │
├──────────────────────────────────────────────────────────┤
│ Native services                                           │
│ Storage (SQLite), dialogs, notifications, network,        │
│ lifecycle, Zig FFI loader                                 │
├──────────────────────────────────────────────────────────┤
│ Native bridge dispatcher                                  │
│ Receives WebView messages, enforces permissions+budgets,  │
│ routes to native services or Zig core, normalizes errors  │
├──────────────────────────────────────────────────────────┤
│ WebView runtime                                           │
│ launcher, sandbox manager, bridge client, permissions,    │
│ resource counters, debug console                          │
├──────────────────────────────────────────────────────────┤
│ Sandboxed generated webapp                                │
│ HTML/CSS/vanilla JS package, no direct platform access    │
└──────────────────────────────────────────────────────────┘

Zig core is a sibling library loaded by the bridge dispatcher,
not a layer in the request stack. The runtime never calls Zig
directly; every core.step request goes through the native
bridge so deterministic actions can be logged and re-emitted
through normal effect paths.
```

## 2. Runtime data flow

```text
Generated webapp
  calls AppRuntime.call(method, params)
        ↓
Sandbox bridge posts message through its assigned MessageChannel port
        ↓
Parent runtime maps port -> (appId, mountToken), validates shape, permission, quota, budget
        ↓
Parent runtime forwards to native bridge dispatcher
        ↓
Native bridge dispatcher applies its own permission/budget check
        ↓
Native bridge dispatches to platform service or Zig core
        ↓
Native bridge returns normalized response
        ↓
Parent runtime returns promise result to generated app
```

The two-stage permission check (runtime + native) is intentional: the runtime check is fast and gives the app a clean error; the native check is the actual security boundary.

## 3. Core step data flow

```text
Webapp event
  ↓
AppRuntime.call("core.step", { event })
  ↓
Runtime sandbox manager (channel-derived appId)
  ↓
Native bridge dispatcher
  ↓
core_step_json(core, input_json)
  ↓
Zig core state machine
  ↓
Actions JSON
  ↓
Native bridge logs (event, actions) to DB if in dev/test session
  ↓
Bridge returns actions to runtime
  ↓
Runtime delivers actions to webapp (which may render or react)
```

## 4. Generated app execution modes

### v0.1 default: sandboxed iframe

The generated app runs inside a sandboxed iframe.

Required iframe sandbox permissions:

```html
sandbox="allow-scripts"
```

Do not grant `allow-same-origin` by default. The generated app communicates only through its assigned MessageChannel port (docs/03 §2.1).

Per-app iframe attributes:

- `allow=""` (no feature policy delegations).
- `csp="..."` set to the runtime CSP (docs/07 §8) so the iframe can refuse the runtime tries to relax it.
- `referrerpolicy="no-referrer"`.

### Future mode: trusted bundled apps

First-party bundled apps may run with looser CSP after explicit review, but v0.1 keeps the same bridge contract for simplicity. The trusted mode does not skip the runtime/native dual permission check.

## 5. Native bridge responsibilities

The native shell owns:

- WebView construction.
- Runtime HTML loading.
- JS/native bridge binding via the platform-appropriate mechanism (docs/05 §1.1).
- Local file access to bundled runtime and examples.
- Persistent storage via `PlatformDatabase` (SQLite).
- File dialogs.
- Notifications/toasts.
- Network requests (with `manifest.networkPolicy` enforcement).
- Platform lifecycle (suspend/resume/background).
- Zig core library loading.
- Crash/error logging.

The web runtime owns:

- App registry UI.
- App package validation (delegated to reference-host validator module).
- Sandbox creation and `mount_token` issuance.
- App lifecycle inside the WebView.
- Permission check before calling native.
- Resource quotas and budget counters.
- Bridge request/response tracking.
- Debug console.

Zig core owns:

- Deterministic state.
- Domain decisions.
- Validation.
- Protocol/state-machine logic.
- Event-to-action mapping.

## 6. App package lifecycle

```text
Generated source package arrives
  ↓
Validate package file list, manifest, policies, capabilities, network policy, budgets
  ↓
Run static HTML/CSS/JS policy checks
  ↓
Accessibility preflight
  ↓
Canonicalize and hash (docs/17 §6)
  ↓
Sign (docs/17 §5)
  ↓
Begin DB transaction (docs/27 §6)
  ↓
  Insert apps / app_versions / app_files / app_permissions / app_install_reports / app_installations
  ↓
  Run migrations if dataVersion increased (docs/19)
  ↓
Commit
  ↓
Run smoke tests in sandbox with mocked bridge
  ↓
Activate via apps.active_install_id, OR quarantine on failure
```

## 7. Bridge message protocol

### Request

```json
{
  "id": "req_123",
  "method": "storage.get",
  "params": { "key": "notes-lite:notes" },
  "timestamp": 1730000000000
}
```

`appId` is **not** included in the request body — the runtime/native bridge derive it from the channel (docs/03 §2.1). Logs may attach `appId` for observability.

### Response

```json
{ "id": "req_123", "ok": true, "result": { "value": [] } }
```

### Error response

```json
{
  "id": "req_123",
  "ok": false,
  "error": {
    "code": "permission_denied",
    "message": "App notes-lite cannot call dialog.openFile",
    "details": {}
  }
}
```

## 8. Storage model **[v0.4]**

Every app receives a namespace based on app id:

```text
<app-id>:<key>
```

Generated apps must not store unprefixed keys. The runtime enforces this even if the generated app violates the rule, and the native bridge enforces it again as a second line of defense.

All hosts use SQLite for the platform database (docs/27, docs/28). JSON-file storage and key-value backends like SharedPreferences are not supported in v0.4. Earlier drafts allowed per-platform key-value stores; that path is closed.

| Platform | Storage backend |
|---|---|
| iOS / macOS | SQLite in Application Support |
| Android | SQLite in app-private storage |
| Windows | SQLite under LocalAppData |
| Linux | SQLite under XDG data home |
| Reference host | SQLite (in-memory by default, file-backed on request) |
| Server | SQLite for dev, Postgres-compatible logical schema for production |

## 9. Network model **[v0.1, hardened v0.3]**

Generated apps do not call `fetch` directly. They call:

```js
AppRuntime.call("network.request", {...})
```

The runtime and native host together enforce:

- `network.request` permission.
- `manifest.networkPolicy` — origin/method/header allowlist.
- Max request body size.
- Max response body size.
- Timeout.
- Redirect handling: redirects to origins outside `networkPolicy.allow` are rejected.

## 10. App runtime modules

Recommended runtime modules:

```text
runtime/
  index.html
  runtime.js
  bridge.js
  app-registry.js
  manifest-validator.js
  sandbox-manager.js          # also issues mount_token, owns MessageChannel
  permission-manager.js
  quota-manager.js
  budget-meter.js             # v0.3 resource budget counters
  network-policy.js           # v0.3 client-side preflight
  capability-client.js        # v0.3 runtime.capabilities wrapper
  storage-client.js
  core-client.js
  components/
    app-shell.js
    app-button.js
    app-dialog.js
    app-table.js
    app-toast.js
  debug/
    console.js
    bridge-inspector.js
    event-timeline.js
```

## 11. Failure philosophy

Every boundary returns structured errors. Do not throw opaque platform errors into generated apps.

Common error codes are listed in docs/03 §5.

## 12. Crash, freeze, and resource exhaustion handling **[v0.3]**

When the WebView process crashes:

- The host writes a `runtime_sessions` row with `outcome = "crashed"`.
- The host shows the user a remount banner with options: reload, switch app, file bug.
- Auto-remount happens only when the previous mount completed `runtime.ready`.
- A crash inside the first 5 seconds of mount counts as a soft failure; three within 24 hours quarantine the installed version.

When the bridge dispatcher detects:

- **Quota exhaustion** (`quota_exceeded` or `RESOURCE_BUDGET_EXCEEDED`): the offending app frame receives the error; the runtime increments a violation counter; three violations in 60 s quarantine the installed version.
- **`core.step` timeout** (default 2000 ms): the bridge returns `timeout` to the app. The Zig core is not killed; subsequent calls remain valid. Repeated timeouts (10 within 60 s) mark the app as `degraded` and log an `app.error` event.
- **Storage failure**: returned as `storage_error`. If it occurs during install, the install transaction is rolled back and the install report records the cause.

## 13. Codex control architecture **[v0.2]**

Codex controls the platform through a developer-only side channel, not through production app APIs.

```text
Codex CLI / IDE extension
  ↓ MCP tool calls
Codex Platform Control Plugin
  ↓ stdio MCP server
codex-platform-mcp
  ↓ HTTP control protocol (token-authenticated)
Platform Dev Control Plane (compile-out in production)
  ↓ native host adapter
Native host app dev build
  ↓ WebView runtime control API
Sandboxed generated webapp
  ↓ AppRuntime.call(...)
Native bridge
  ↓ core_step_json(...)
Zig core
```

The control plane is intentionally separate from `AppRuntime.call`.

- `AppRuntime.call` is the user-facing generated app bridge.
- The control plane is the developer/test bridge used by Codex, Playwright-style tests, and platform smoke tests.

This separation prevents test-only powers from becoming available to generated apps.

### 13.1 Dev control plane responsibilities

The control plane must expose capabilities for:

- Host lifecycle: launch, attach, reload, stop.
- App package management: install, list, open, uninstall, reset.
- Runtime inspection: current app, route, permissions, component tree, active modal, focus, errors.
- DOM interaction: query, click, type, key press, drag, screenshot.
- Bridge inspection: request log, response log, permission denials, latency.
- Zig core inspection: event/action log, current deterministic snapshot, replay.
- Effect control: storage, network mocks, dialog mocks, timer control, notification capture.
- Assertion helpers: visible text, selector state, bridge call occurred, storage key exists, core action emitted.

### 13.2 Platform adapter notes

- macOS, Windows, and Linux desktop hosts expose the control plane over `127.0.0.1` in dev builds only.
- Android emulator builds support host connection through the Android debug bridge adapter.
- iOS simulator builds support a simulator adapter with an app-initiated control session handshake.
- Physical mobile devices may require a manual tunnel, pairing token, or be excluded from v0.1 automation.
- Production builds compile out the control server and reject all control-plane launch flags.

### 13.3 Control plane authority model

The control plane is powerful enough for Codex to test everything, but only in dev/test builds.

Required gates:

- Dev build flag.
- Local-only binding by default (`127.0.0.1`).
- Per-launch session token (32-byte URL-safe random) — see docs/14 §Authentication.
- Optional user approval for destructive operations.
- Audit log of every control command (stored in `control_commands`).
- Host rejects control commands when the runtime is not in test mode.

## 14. Persistence layer integration **[v0.4]**

```text
Generated webapp
  -> AppRuntime.call("storage.*")
  -> runtime permission/prefix/quota checks
  -> native bridge permission check
  -> PlatformDatabase storage repository
  -> SQLite or Postgres logical schema
  -> app_storage(app_id, key, value_json)
```

The same database also stores app registry, app package files, app permissions, app install reports, version history, rollback events, migrations, runtime sessions, bridge/core logs, snapshots, micro-test runs, control commands, mocks, and backup/export data (docs/27).

### 14.1 Database-backed install lifecycle

```text
validate package
  -> canonicalize / hash
  -> begin transaction
       insert apps
       insert app_versions
       insert app_files
       insert app_permissions
       insert app_install_reports
       insert app_installations
       optional: run migrations (with pre-migration snapshot)
  -> commit
  -> activate app version
  -> runtime mount gate reads active install
```

### 14.2 Database-backed debug lifecycle

```text
Codex / test runner
  -> insert control_sessions
  -> insert control_commands (one per tool call)
  -> bridge_calls (every test bridge dispatch)
  -> core_events / core_actions
  -> runtime_snapshots
  -> test_runs
  -> optional: db.export_debug_bundle
```
