# Terrane MCP Capability Operations

Direct capability operation is an advanced path. App-building should normally
use `app_scaffold`, `app_register_inline`, `app_register`, `app_actions`, and
`invoke`.

## Capability Docs

Capability docs are owned by capability crates, not by `host/mcp`.

Use one of:

```json
{"name":"capability_info","arguments":{"namespace":"kv","format":"json"}}
```

or:

```json
{"uri":"terrane://capabilities/kv"}
```

The capability doc is expected to include commands, queries, events, resource
methods, params, returns, errors, examples, limits, compatibility notes, and
internal notes when `includeInternal` is explicitly requested.

## Read Path

Use `capability_query` for reads.

```json
{
  "name": "capability_query",
  "arguments": {
    "capability": "app",
    "query": "exists",
    "args": ["notes-demo"]
  }
}
```

Queries must not append records, run effects, or touch runtime paths.

## Command Path

Use `capability_command` only after reading help.

```json
{
  "name": "capability_command",
  "arguments": {
    "name": "app.add",
    "help": true
  }
}
```

Then dry-run when supported.

```json
{
  "name": "capability_command",
  "arguments": {
    "name": "app.add",
    "args": ["notes-demo", "Notes Demo"],
    "dryRun": true
  }
}
```

Effect and runtime commands can reject dry-run. Treat that rejection as a guard,
not as a reason to force the command.

## Safer Alternatives

- Use `app_register_inline` instead of raw `app.add` for generated app files.
- Use `app_register` instead of raw `app.add --source` for existing bundles.
- Use `app_actions` and `invoke` instead of runtime capability commands.
- Use `capability_query` instead of commands for state inspection.
