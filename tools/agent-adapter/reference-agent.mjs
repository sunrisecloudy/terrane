#!/usr/bin/env node
import { fileURLToPath } from "node:url";
import { catalogToTools } from "./catalog-to-tools.mjs";
import { executeTool } from "./execute-tool.mjs";
import { loadCatalog } from "./lib/catalog.mjs";
import { defaultCatalogPath } from "./lib/paths.mjs";

const NOTES_LITE_INTENT = "list my notes";

/**
 * Scripted reference agent loop for the notes-lite demo path.
 *
 * 1. Load the role/tier-scoped catalog (fixture file or live system.describe).
 * 2. Project public tools for the agent.
 * 3. Answer "list my notes" via query_execute against the notes collection.
 */
export async function runReferenceAgentLoop(options = {}) {
  const tier = options.tier ?? "public";
  const role = options.role ?? "owner";
  const catalog = await loadCatalog({
    catalogPath: options.catalog ?? defaultCatalogPath,
    describe: options.describe ?? false,
    server: options.server ?? null,
    token: options.token ?? null,
    tier,
    role,
    workspaceId: options.workspaceId ?? "reference-agent",
    actor: options.actor ?? "reference-agent",
  });

  const projection = catalogToTools(catalog, { tier, role });
  const toolNames = projection.tools.map((tool) => tool.name).sort();
  if (!toolNames.includes("query_execute")) {
    throw new Error(`reference agent expected query_execute in offered tools: ${toolNames.join(", ")}`);
  }

  const queryResult = await executeTool(
    "query_execute",
    { collection: "notes" },
    {
      catalogDocument: catalog,
      tier,
      role,
      dryRun: options.dryRun ?? false,
      confirm: options.confirm ?? false,
      server: options.server ?? null,
      token: options.token ?? null,
      workspaceId: options.workspaceId ?? "reference-agent",
      actor: options.actor ?? "reference-agent",
      requestId: options.requestId ?? "reference-agent-notes",
    },
  );

  return {
    intent: NOTES_LITE_INTENT,
    catalogVersion: projection.catalogVersion,
    tools: toolNames,
    reverseMap: projection.reverseMap,
    queryResult,
  };
}

function parseArgs(argv) {
  const options = {
    catalog: defaultCatalogPath,
    tier: "public",
    role: "owner",
    describe: false,
    server: null,
    token: process.env.FORGE_SERVER_TOKEN ?? null,
    workspaceId: "reference-agent",
    actor: "reference-agent",
    dryRun: false,
    confirm: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--catalog") options.catalog = argv[++index];
    else if (arg === "--tier") options.tier = argv[++index];
    else if (arg === "--role") options.role = argv[++index];
    else if (arg === "--describe") options.describe = true;
    else if (arg === "--server") options.server = argv[++index];
    else if (arg === "--token") options.token = argv[++index];
    else if (arg === "--workspace") options.workspaceId = argv[++index];
    else if (arg === "--actor") options.actor = argv[++index];
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--confirm") options.confirm = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown option: ${arg}`);
  }

  return options;
}

function printUsage() {
  console.log(`Usage: node tools/agent-adapter/reference-agent.mjs [options]

Run the scripted notes-lite reference loop:
  system.describe/catalog -> project tools -> query_execute(collection=notes)

Options:
  --catalog <path>     Catalog JSON (default: forge/data/commands.json)
  --describe           Load catalog via system.describe (local core-invoke)
  --server <url>       Fetch/execute via POST /bridge
  --token <token>      Bearer token for --server
  --tier <tier>        Agent tier ceiling (default: public)
  --role <role>        Agent role (default: owner)
  --workspace <id>     Workspace id in CoreCommand envelope
  --actor <id>         Actor id in CoreCommand envelope
  --dry-run            Validate + print envelope without executing query.execute
  --confirm            Allow mutating/effectful commands (unused in this loop)
  --help               Show this message
`);
}

async function cli(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    printUsage();
    return;
  }

  const result = await runReferenceAgentLoop(options);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  if (!result.queryResult.ok) {
    process.exit(1);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  cli().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}