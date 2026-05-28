# Codex Platform MCP Server

This tool connects Codex to the platform dev control plane over MCP stdio.

## Responsibility

The MCP server exposes typed tools to Codex. It should not implement business logic. It forwards tool calls to a platform control plane and normalizes results.

```text
Codex -> MCP server -> dev control plane -> native host/runtime/Zig core
```

## Environment

- `PLATFORM_CONTROL_URL` — base URL, default `http://127.0.0.1:7878`.
- `PLATFORM_CONTROL_TOKEN_FILE` — optional path to the per-launch token file.
- `PLATFORM_CONTROL_TOKEN` — explicit test/dev override. If unset, the server reads the token file and fails fast when it is missing or empty.
- `PLATFORM_CONTROL_DEFAULT_TARGET` — default target, usually `fake-host` or `macos`.

## Required tool groups

- Lifecycle: `platform.health`, `platform.list_targets`, `platform.launch`, `platform.stop`.
- App packages: `platform.validate_package`, `platform.install_webapp_package`, `platform.open_webapp`, `platform.approve_webapp_update`, `platform.reset_webapp`.
- UI: `runtime.snapshot`, `runtime.query`, `runtime.click`, `runtime.type`, `runtime.set_value`, `runtime.press_key`, `runtime.wait_for`, `runtime.screenshot`.
- Logs: `runtime.console_logs`, `runtime.bridge_calls`, `runtime.event_log`, `runtime.clear_logs`.
- Effects: `runtime.storage_get`, `runtime.storage_set`, `runtime.storage_reset`, `runtime.network_mock_set`, `runtime.dialog_mock_set`, `runtime.timer_advance`.
- Core/replay: `runtime.core_step`, `runtime.core_snapshot`, `runtime.replay_events`.
- Assertions: `runtime.assert_visible`, `runtime.assert_text`, `runtime.assert_bridge_call`, `runtime.assert_no_console_errors`, `runtime.run_microtest`, `runtime.run_smoke_tests`, `platform.run_platform_smoke`.

## Implementation notes

The current implementation is dependency-free and speaks the MCP JSON-RPC
stdio framing directly. Each declared tool forwards mechanically to
`POST /control/command` on the configured platform control URL.

Run:

```sh
npm test
npm start
```

Start with a fake host and contract tests. Then attach to real desktop hosts.
Mobile simulator/emulator adapters can come later.

## v0.4 database tool group

Add safe DB inspection tools:

- DB: `db.snapshot`, `db.query_app_storage`, `db.query_app_versions`, `db.query_bridge_calls`, `db.query_core_events`, `db.query_test_runs`, `db.export_debug_bundle`.

These tools forward to the dev control plane. They must not execute arbitrary SQL by default.
