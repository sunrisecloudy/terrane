# Forge CLI Reference

The `forge` binary (`forge-cli`) is a catalog-driven front-end over the same `WorkspaceCore::handle` facade used by native shells and `forge-server`.

## Subcommands

| Subcommand | Purpose |
| --- | --- |
| `forge commands` | List commands from `system.describe` (supports `--tier`, `--namespace`, `--include-inner`, `--json`). |
| `forge describe <name>` | Show one descriptor plus payload/response schema paths. |
| `forge run <name>` | Execute any outer command locally or against `--server <url>`. |
| `forge trace <run_id>` | Read `system.trace` for a recorded run (redacted host-call journal). |
| `forge demo` | M0a acceptance spine: install `notes-lite`, run, assert byte-identical replay. |
| `forge help` | Catalog-generated help grouped by namespace. |

## Common flags

| Flag | Applies to | Meaning |
| --- | --- | --- |
| `--json` | all | Emit machine-readable JSON. |
| `--workspace <path>` | run, trace, commands | Open a file-backed workspace at the given path. |
| `--in-memory` | run, trace, commands | Ephemeral workspace (tests and one-shot runs). |
| `--payload <json>` / `--file <path>` | run | Command payload. |
| `--actor <id>` / `--role <role>` | run, commands | Actor identity for RBAC. |
| `--dry-run` | run | Validate envelope only; do not dispatch. |
| `--events` | run | After success, drain and print emitted `CoreEvent`s. |
| `--server <url>` | run | POST the envelope to `forge-server` `/bridge`. |
| `--token <bearer>` | run, server | Bearer token (or `FORGE_SERVER_TOKEN` env). |

## Examples

```sh
# Discover the catalog
forge commands --tier operator

# Install and run an example applet
forge run applet.install --payload '{"applet_id":"notes-lite",...}'
forge run runtime.run --applet-id notes-lite --payload '{"input":{"title":"Buy milk"}}'

# Trace host calls for a run
forge trace <run_id> --json

# CI spine gate
forge demo
```

## Tests

```sh
cd forge
cargo test -p forge-cli --test cli_unified --locked
cargo test -p forge-cli --test bundled_apps_cli_e2e --locked
cargo test -p forge-cli --test forge_examples --locked
```