# Codex Platform Control Plugin

## Purpose

The Codex control plugin lets Codex connect to the platform during development and control it at micro-test granularity.

It is required because AI-generated apps need a tight repair loop:

```text
generate package
  ↓
validate package
  ↓
install into dev host
  ↓
open inside WebView runtime
  ↓
click/type/inspect/assert
  ↓
read logs/bridge/core/storage state
  ↓
repair package
  ↓
retest
```

This is a developer tool, not a production feature.

## Codex integration shape

Use a Codex plugin that packages:

- Skills that instruct Codex how to generate, validate, test, repair, and replay generated apps.
- A Model Context Protocol server that exposes platform control tools.
- Optional hooks later for loading platform context when a Codex session starts.

Recommended plugin path:

```text
codex-plugin/platform-control/
  .codex-plugin/plugin.json
  .mcp.json
  skills/
    platform-micro-test/SKILL.md
    generated-webapp-repair/SKILL.md
    core-replay-debug/SKILL.md
```

## Control path

```text
Codex
  -> MCP tool call
  -> codex-platform-mcp local server
  -> dev control plane over localhost/tunnel
  -> native host dev build
  -> WebView runtime dev hooks
  -> sandboxed generated app
  -> native bridge
  -> Zig core
```

Codex should never call generated app APIs directly. It calls MCP tools. The MCP server calls the control plane. The control plane calls runtime/native/Zig adapters.

## Tool groups

### Lifecycle tools

| Tool | Purpose |
|---|---|
| `platform.health` | Verify MCP server and host control plane are reachable. |
| `platform.list_targets` | List configured host targets: macos, ios-simulator, android-emulator, windows, linux, server. |
| `platform.launch` | Launch or attach to a dev host. |
| `platform.stop` | Stop a host session. |
| `platform.reload_runtime` | Reload the WebView runtime. |

### App package tools

| Tool | Purpose |
|---|---|
| `platform.validate_package` | Validate generated package shape, manifest, bridge usage, and policy. |
| `platform.install_webapp_package` | Install a package into the host app registry. |
| `platform.list_webapps` | List installed/bundled generated apps. |
| `platform.open_webapp` | Open an app by id. |
| `platform.uninstall_webapp` | Remove an app in dev mode. |
| `platform.reset_webapp` | Clear app state/storage/logs. |

### UI tools

| Tool | Purpose |
|---|---|
| `runtime.snapshot` | Return current screen, route, DOM summary, accessibility tree, active app, errors. |
| `runtime.query` | Query DOM elements by `data-testid`, role, text, or selector. |
| `runtime.click` | Click/tap an element. |
| `runtime.type` | Type into an input. |
| `runtime.set_value` | Set form value without relying on key events. |
| `runtime.press_key` | Send keyboard event. |
| `runtime.drag` | Simulate drag/drop where supported. |
| `runtime.screenshot` | Capture current rendered screen. |
| `runtime.wait_for` | Wait for selector/text/event/bridge call/runtime idle. |

### Runtime/bridge/core tools

| Tool | Purpose |
|---|---|
| `runtime.console_logs` | Read console logs and errors. |
| `runtime.bridge_calls` | Read bridge request/response log. |
| `runtime.clear_logs` | Clear console/bridge/event logs. |
| `runtime.call_bridge` | Call a bridge method as a test harness, subject to manifest permissions. |
| `runtime.core_step` | Send a test event into Zig core through the normal bridge path. |
| `runtime.core_snapshot` | Read deterministic core state snapshot if dev mode enables it. |
| `runtime.event_log` | Read runtime event/action log. |
| `runtime.replay_events` | Replay a captured event log and compare output. |

### Effect control tools

| Tool | Purpose |
|---|---|
| `runtime.storage_get` | Read namespaced app storage. |
| `runtime.storage_set` | Seed namespaced app storage. |
| `runtime.storage_reset` | Clear app storage. |
| `runtime.network_mock_set` | Mock `network.request` result. |
| `runtime.network_mock_reset` | Clear network mocks. |
| `runtime.dialog_mock_set` | Mock `dialog.openFile` / `dialog.saveFile`. |
| `runtime.notification_capture` | Read captured notification/toast calls. |
| `runtime.timer_advance` | Advance fake timers in test mode. |
| `runtime.fault_inject` | Inject storage/network/core permission failures. |

### Assertion tools

| Tool | Purpose |
|---|---|
| `runtime.assert_visible` | Assert selector or text is visible. |
| `runtime.assert_text` | Assert text content. |
| `runtime.assert_bridge_call` | Assert a bridge call happened with matching payload. |
| `runtime.assert_no_console_errors` | Assert no console errors. |
| `runtime.assert_storage` | Assert storage value. |
| `runtime.assert_core_action` | Assert core action appeared in the log. |
| `runtime.run_microtest` | Execute a `.microtest.json` file. |
| `runtime.run_smoke_tests` | Execute app package `smoke-tests.json`. |

## Required MCP tool behavior

All tools must return structured JSON:

```json
{
  "ok": true,
  "result": {},
  "diagnostics": {
    "target": "macos",
    "sessionId": "session_...",
    "appId": "notes-lite",
    "timestamp": "2026-05-28T00:00:00Z"
  }
}
```

Errors must be machine-readable:

```json
{
  "ok": false,
  "error": {
    "code": "selector.not_found",
    "message": "No element matched data-testid=new-note-button",
    "details": {}
  },
  "diagnostics": {}
}
```

## Safety gates

The control plugin must enforce:

- Local/dev mode only.
- Session token required (see "Authentication" below).
- Every target has a declared platform id.
- Destructive operations require `confirm: true`.
- No production build may expose `/sessions` or runtime test hooks.
- Arbitrary JS evaluation must be absent by default. If implemented for debugging, it must be named `runtime.unsafe_eval`, disabled by default, and blocked in CI.

## Authentication

The control plane uses a per-launch session token. Spec:

| Field | Value |
|---|---|
| Token size | 32 bytes, cryptographically random |
| Encoding | URL-safe base64 (no padding), 43 chars |
| Header | `X-Platform-Control-Token: <token>` |
| Where written | `$XDG_RUNTIME_DIR/native-ai-webapp/control.token` (Linux), `~/Library/Application Support/<bundle>/control.token` (macOS), `%LOCALAPPDATA%\<product>\control.token` (Windows). File mode `0600` on POSIX; ACL restricted to current user on Windows. |
| Rotation | New token on every host launch. Previous file truncated before write. |
| Lifetime | Until the host process exits. No renewal endpoint. |
| Bind address | `127.0.0.1` only in dev builds. Production builds compile the listener out entirely. |
| Refusal mode | Any request without the header or with a non-matching token returns HTTP 401 with `{"code":"control_auth_required"}`. Three failures from the same connection trigger a 60-second connection ban logged to `control_commands`. |
| Transport | Plain HTTP over loopback is acceptable; TLS is required only if the bind address is changed (e.g., to support an emulator adapter). |
| Allowed methods | `GET /health`, `POST /sessions`, `DELETE /sessions/:id`, `GET /sessions/:id/snapshot`, `POST /sessions/:id/command`, `GET /sessions/:id/events`. All other paths return 404. |
| Audit | Every accepted and rejected request writes a row in `control_commands` with `(timestamp, method, path, decision, error_code)`. |

The Codex MCP server reads the token file at startup, fails fast if the file does not exist or is unreadable, and keeps the token in process memory only — it never writes it to the MCP transcript or to logs.

### Authentication failure modes

| Symptom | Likely cause | Action |
|---|---|---|
| `control_auth_required` | Token file missing or stale | Restart the host; the MCP server picks up the new token |
| 403 `not_in_dev_mode` | Host is not in dev mode | Re-launch with `--dev` (or the platform's dev build) |
| Connection refused | Production build, control plane compiled out | Use a dev/TestFlight/Developer-ID build instead |
| Connection banned | Three failed attempts | Wait 60 s; check for stale token in MCP cache |

## Why MCP instead of plain shell commands

MCP gives Codex named tools with typed inputs and structured outputs. That allows Codex to test generated apps without inventing ad-hoc shell commands or parsing logs.

The plugin still uses normal project tests and shell commands for build/lint/unit tests. MCP is specifically for controlling the running platform.

## v0.3 additional tools

### Trust/install tools

| Tool | Purpose |
|---|---|
| `platform.sign_webapp_package` | Canonicalize, hash, and sign a validated source package. |
| `platform.install_report` | Return the latest install report for an app/version. |
| `platform.list_webapp_versions` | List immutable installed versions and statuses. |
| `platform.approve_webapp_update` | Activate an installed version whose report requires user approval. |
| `platform.rollback_webapp` | Roll back active app to a previous version. |
| `platform.quarantine_webapp` | Mark an installed version unsafe. |

### Capabilities/snapshot/resource tools

| Tool | Purpose |
|---|---|
| `runtime.capabilities` | Return current runtime capability document. |
| `platform.create_snapshot` | Capture runtime/app/core/storage/debug state. |
| `platform.restore_snapshot` | Restore a snapshot into a dev host. |
| `runtime.compare_snapshot` | Compare current state against a snapshot/replay baseline. |
| `runtime.resource_usage` | Return current resource usage and budget status. |

### Audit tools

| Tool | Purpose |
|---|---|
| `runtime.run_accessibility_audit` | Produce accessibility report. |
| `runtime.accessibility_snapshot` | Return accessible tree/roles/names/focus state. |
| `platform.run_policy_audit` | Run static package policy checks. |
| `platform.run_repair_loop` | Execute the standard generate/validate/install/test/repair loop in dev mode. |

## Database inspection tools

The Codex plugin must expose safe database inspection tools through the dev control plane:

```text
db.snapshot
db.query_app_storage
db.query_app_versions
db.query_bridge_calls
db.query_core_events
db.query_test_runs
db.export_debug_bundle
```

These tools are read-only except `db.export_debug_bundle`, which creates an export/debug artifact. They must not expose arbitrary SQL. Unsafe SQL can exist only behind a separate explicit unsafe dev-mode setting and must not be enabled by default.

The DB tools let Codex verify micro-level behavior that is hard to see from DOM alone:

- whether storage bridge calls wrote expected rows;
- whether rollback changed the active install pointer;
- whether core events/actions persisted;
- whether bridge errors were logged;
- whether migrations produced expected app_storage changes;
- whether test runs wrote diagnostics.
