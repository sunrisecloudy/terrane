# Phase 3 — The generic `forge` CLI front-end

**Theme:** grow `forge-cli` from `demo`-only into a thin, generic front-end over
the facade. This is the deliverable the request literally asked for: "a CLI that
can run every action," and self-describing so an agent can learn it.

**Risk:** low. New argv surface over existing library functions (`handle`,
`list_records`) and the new `system.describe`.
**Replay impact:** none.

## Command surface

```
forge commands [--tier T] [--namespace NS] [--json]
        List commands (from system.describe), grouped by namespace, filtered.

forge describe <name> [--json]
        Print one command: summary, payload/response schema, roles, tier,
        mutates/effectful flags, events, examples.

forge run <name> [--payload <json> | --payload - | --file <path>]
                 [--workspace <id>] [--actor <id>] [--role <role>]
                 [--server <url>] [--json] [--dry-run]
        Issue a command and print the CoreResponse.

forge demo                       # unchanged M0a spine gate
forge help | --help | -h         # usage (now generated from the catalog)
```

### Behavior details

- **`run` builds the envelope** (`request_id`, `actor`, `workspace_id`, `name`,
  `payload`) — the same shape every shell builds (F5), now in one Rust place.
- **Transport:** default opens a local core directly via `forge-core`
  (persistent path or in-memory with `--in-memory`); `--server <url>` POSTs to
  `/bridge` instead (`forge/crates/server/src/lib.rs:92`). Bearer token via
  `--token` / `FORGE_SERVER_TOKEN`.
- **`--payload -`** reads JSON from stdin (pairs with the existing `core-invoke`
  ergonomics); `--file` reads from disk.
- **`--dry-run`** validates the payload against the command's `payload_schema`
  and prints the envelope *without* executing — invaluable for agents.
- **`run` refuses `surface: "inner"`** commands (`ctx.*`) with a helpful message
  pointing at the app runtime — operators don't issue host-calls directly.
- **Exit codes:** `0` on `ok:true`; non-zero on `ok:false` or transport error,
  with `CoreError` printed to stderr (mirrors `forge demo`'s gate-friendly exit,
  `main.rs:28`).
- **`--json`** everywhere for machine consumption (agents, scripts).

## Steps

### P3.1 — Arg parser

Replace the hand-rolled `match` in `main.rs` (`cli/src/main.rs:14`) with a small
subcommand parser. Prefer keeping the dependency footprint consistent with the
workspace; if `clap` is already a dep elsewhere use it, else a minimal parser
keeps `forge-cli` light. Decide in [13](13-OPEN-QUESTIONS.md) Q3.

### P3.2 — `commands` / `describe`

Both call `system.describe` (Phase 2) against an opened core and render the
catalog. `describe` pretty-prints one descriptor + its schemas. With `--json`
they emit the raw catalog so tools/agents consume it directly.

### P3.3 — `run`

Wrap the existing `forge_cli::handle` (`cli/src/lib.rs:186`): parse `--payload`,
build the command, dispatch (local or `--server`), print response. Add
`--dry-run` schema validation using the command's `payload_schema`.

### P3.4 — Help generated from the catalog

`forge help` lists namespaces and commands from the catalog so usage never drifts
from reality. Keep the `demo` description.

### P3.5 — Tests

Extend `forge/crates/cli/tests/` (`e2e.rs`, `scenarios.rs`):

- `forge commands --json` lists the same set `system.describe` returns.
- `forge run query.execute --payload …` round-trips against a seeded core.
- `forge run applet.install` installs (reuse the demo applet fixture).
- `forge run <inner>` is rejected.
- `forge run <unknown>` returns the CR-A5 validation error and non-zero exit.
- `--dry-run` rejects a payload that violates the schema.

## Example sessions

```sh
$ forge commands --namespace applet
applet.install     ✎ operator   Install an applet from a manifest + sources.
applet.enable      ✎ operator   Enable an installed applet.
applet.suspend     ✎ operator   Suspend an active applet.
applet.upgrade     ✎ operator   Atomically upgrade an active applet.
applet.uninstall   ✎ operator   Uninstall an applet.

$ forge describe query.execute
query.execute  (public, read-only)
  Roles: Owner, Maintainer, Editor, Viewer, Auditor
  Payload:  schemas/commands/query.execute.request.schema.json
            { "collection": string, "filter"?: object, "limit"?: int }
  Response: { "rows": [{ "id": string, "fields": object }] }

$ forge run query.execute --payload '{"collection":"notes"}' --json
{ "ok": true, "payload": { "rows": [ /* … */ ] } }

$ echo '{"collection":"notes"}' | forge run query.execute --payload -
```

## Deliverables

- `forge commands`, `forge describe`, `forge run` with local + `--server`
  transport.
- Catalog-generated help.
- CLI e2e tests covering list/describe/run/reject/dry-run.

## Validation

```sh
cd forge
cargo test -p forge-cli
cargo clippy -p forge-cli -- -D warnings
cargo run -p forge-cli -- commands
cargo run -p forge-cli -- run query.execute --payload '{"collection":"notes"}'
cargo run -p forge-cli -- demo          # still green
```

## Exit criteria

- Every outer command in the catalog is runnable via `forge run`.
- `forge commands`/`describe` reflect the live catalog with zero hard-coded
  command knowledge in the CLI.
- The `demo` gate is unchanged and green.
