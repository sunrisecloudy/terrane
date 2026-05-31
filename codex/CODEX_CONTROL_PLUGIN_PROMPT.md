# Codex prompt: implement control plugin

Implement the developer-only Codex control plugin and reference host first.

Requirements:

1. Read `docs/14_CODEX_CONTROL_PLUGIN.md`, `docs/15_MICRO_TESTING_PROTOCOL.md`, and `docs/16_CODEX_PLUGIN_IMPLEMENTATION_PLAN.md`.
2. Implement `tools/reference-host` enough to accept `/health` and `/command`.
3. Implement `tools/codex-platform-mcp` using the current MCP SDK.
4. Expose `platform.health`, `platform.launch`, `platform.install_webapp_package`, `platform.open_webapp`, `runtime.snapshot`, `runtime.click`, `runtime.type`, `runtime.assert_visible`, `runtime.bridge_calls`, `runtime.run_microtest`, and `runtime.assert_no_console_errors` first.
5. Add schema validation for control commands and micro-test files.
6. Add tests that run `tests/micro/notes-lite-create-note.microtest.json` against the reference host.
7. Do not implement native platform adapters until the reference host contract passes.

After implementation, provide:

- Commands to run the reference host.
- Commands to run the MCP server.
- Commands to run micro-tests.
- Any Codex config or plugin install steps needed.
