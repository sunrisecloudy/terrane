# AI Native WebView Platform Control Plugin

This is a local Codex plugin for development control and repair workflows.

It packages:

- `skills/` — Codex workflows for micro-testing, repairing generated apps, and replay-debugging Zig core behavior.
- `.mcp.json` — MCP server configuration for `tools/codex-platform-mcp`.
- `.codex-plugin/plugin.json` — plugin metadata.

## Local install concept

Add a repo-local marketplace entry at `.agents/plugins/marketplace.json` pointing to this plugin. Then install/enable it from Codex's plugin UI or CLI plugin browser.

The MCP server path in `.mcp.json` is resolved from this plugin root and points back to the repository checkout at `../../tools/codex-platform-mcp/src/server.js`.

The MCP server reads the per-launch control token from the platform token file
unless `PLATFORM_CONTROL_TOKEN` is explicitly set by a test harness. Do not
check a shared control token into this plugin config.

## Dev-only warning

This plugin controls dev/test builds. Do not expose its control endpoints in production.
