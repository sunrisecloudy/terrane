# Terrane MCP Security

Terrane MCP operates one local `TERRANE_HOME`. Treat it as a local admin surface
unless the host is explicitly wrapped by auth and policy.

## Permission Model

Clients should grant the model only the tools required for the task. A strong
locked-down app-building client can deny file reads, filesystem listing, shell,
grep, glob, web fetch, web search, and language-server tools. The MCP-only app
path still works through `app_scaffold` and `app_register_inline`.

## Mutation Rules

Mutating MCP tools must route through core dispatch or a host helper that
dispatches through core. They must not mutate capability state directly.

Examples:

- `app_register_inline` writes owned bundle files, then dispatches `app.add`.
- `app_register` validates a source bundle, then dispatches `app.add`.
- `capability_command` dispatches commands through core.

## Destructive Actions

Commands that remove apps, clear storage, fetch networks, run code, or write
runtime state should be treated as explicit operator actions. Prefer:

1. Read capability docs.
2. Call command help.
3. Dry-run when supported.
4. Commit only when the requested destructive action is explicit.

## Transport Notes

The stdio host writes protocol frames to stdout and diagnostics to stderr. The
HTTP host exposes the same MCP behavior at `POST /mcp` and reuses existing
origin/auth checks.

## Audit Expectations

Tool results should be structured and visible to the model. Tool-level failures
return `isError: true`; malformed protocol requests return JSON-RPC errors.
When possible, errors include a concrete next `tools/call` example.
