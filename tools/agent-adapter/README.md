# Agent adapter (Phase 5)

Projects the Terrane `CommandDescriptor` catalog into LLM tool definitions and
executes tool calls through the same transport as the CLI (`core-invoke` or
`POST /bridge`).

## Modules

| File | Role |
| --- | --- |
| `catalog-to-tools.mjs` | `catalogToTools(catalog, { tier, role })` → `{ tools, reverseMap }` |
| `execute-tool.mjs` | Map tool name → command, validate payload, invoke core |
| `lib/catalog.mjs` | Load/filter catalog by tier, role, and surface |
| `lib/transport.mjs` | Local `core-invoke` and remote `/bridge` transport |

## Catalog sources

1. **`forge/data/commands.json`** (default when present)
2. **`--catalog <path>`** — explicit JSON file (descriptor array or `{ commands: [...] }`)
3. **`--describe`** — fetch via `system.describe` using local `core-invoke`
4. **`--server <url>`** — fetch/execute via `POST /bridge`

`system.describe` responses (`{ ok, payload: { commands } }`) are accepted directly.

## Guardrails

- **Tier ceiling**: projector and executor both filter by `--tier` (`public` < `operator` < `admin` < `debug`).
- **Role filtering**: only commands whose `required_roles` include the agent role are offered.
- **Inner surface refused**: `surface: "inner"` and `ctx.*` reference entries are never emitted or executed.
- **`ui.dispatch_event`**: included at `public` tier like the CLI catalog.
- **Confirm policy**: mutating/effectful tools require `--confirm` or `"confirm": true` in args.
- **Schema pre-validation**: payload is checked against `payload_schema` before transport (like `--dry-run`).

## Examples

### Project public tools from the sample fixture

```sh
node --no-warnings tools/agent-adapter/catalog-to-tools.mjs \
  --catalog tools/agent-adapter/fixtures/sample-catalog.json \
  --tier public \
  --role editor
```

### Project operator-tier tools for an owner agent

```sh
node --no-warnings tools/agent-adapter/catalog-to-tools.mjs \
  --catalog tools/agent-adapter/fixtures/sample-catalog.json \
  --tier operator \
  --role owner \
  | jq '.tools[].name'
```

Expected public tool names:

```text
query_execute
runtime_run
ui_dispatch_event
```

### Dry-run a read tool (validate envelope only)

```sh
node --no-warnings tools/agent-adapter/execute-tool.mjs \
  --catalog tools/agent-adapter/fixtures/sample-catalog.json \
  --tool query_execute \
  --args '{"collection":"notes"}' \
  --tier public \
  --role owner \
  --dry-run
```

### Execute against local core (once `system.describe` / catalog is wired)

```sh
# Discover tools from live core
node --no-warnings tools/agent-adapter/catalog-to-tools.mjs \
  --describe \
  --tier public \
  --role owner

# Run a query
node --no-warnings tools/agent-adapter/execute-tool.mjs \
  --describe \
  --tool query_execute \
  --args '{"collection":"notes"}' \
  --tier public \
  --role owner
```

### Execute via HTTP server

```sh
export FORGE_SERVER_TOKEN=dev-token

node --no-warnings tools/agent-adapter/execute-tool.mjs \
  --server http://127.0.0.1:8787 \
  --tool query_execute \
  --args '{"collection":"notes"}' \
  --tier public \
  --role owner
```

### Mutating tool with confirm

```sh
node --no-warnings tools/agent-adapter/execute-tool.mjs \
  --catalog tools/agent-adapter/fixtures/sample-catalog.json \
  --tool runtime_run \
  --args '{"app_id":"notes-lite","confirm":true}' \
  --tier public \
  --role owner \
  --confirm \
  --dry-run
```

## Programmatic use

```js
import { catalogToTools } from "./catalog-to-tools.mjs";
import { executeTool } from "./execute-tool.mjs";
import { loadCatalogFromFile } from "./lib/catalog.mjs";

const catalog = loadCatalogFromFile("forge/data/commands.json");
const { tools, reverseMap } = catalogToTools(catalog, { tier: "public", role: "editor" });

const result = await executeTool("query_execute", { collection: "notes" }, {
  catalogDocument: catalog,
  tier: "public",
  role: "editor",
  dryRun: true,
});
```

## Tests

```sh
node --test --no-warnings tools/agent-adapter/test/agent-adapter.test.mjs
```

## When `forge/data/commands.json` lands

Phase 1/11 will emit `forge/data/commands.json` from the Rust registry. Until then,
use `--catalog tools/agent-adapter/fixtures/sample-catalog.json` or `--describe`
once `system.describe` is registered in core.