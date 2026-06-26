#!/usr/bin/env node
import { fileURLToPath } from "node:url";
import {
  filterCatalog,
  loadCatalog,
  normalizeCatalog,
} from "./lib/catalog.mjs";
import { buildNameMaps, commandToToolName } from "./lib/names.mjs";
import { resolveInputSchema } from "./lib/schema.mjs";
import { defaultCatalogPath } from "./lib/paths.mjs";

/**
 * Project a CommandDescriptor catalog into LLM tool definitions.
 *
 * @param {object[]|object} catalog - Descriptor array, catalog file contents, or system.describe response
 * @param {object} options
 * @param {string} [options.tier="public"] - Maximum visibility tier for the agent
 * @param {string} [options.role="owner"] - Agent role used for RBAC filtering
 * @param {boolean} [options.includeInner=false] - Include ctx.* / inner-surface reference entries
 * @returns {{ tools: object[], reverseMap: Record<string,string>, catalogVersion: string|null, filteredCount: number, totalCount: number }}
 */
export function catalogToTools(catalog, {
  tier = "public",
  role = "owner",
  includeInner = false,
} = {}) {
  const normalized = normalizeCatalog(catalog);
  const filtered = filterCatalog(normalized.commands, { tier, role, includeInner });
  const { toolToCommand } = buildNameMaps(filtered);

  const tools = filtered.map((command) => ({
    name: commandToToolName(command.name),
    description: buildToolDescription(command),
    input_schema: resolveInputSchema(command),
    _meta: {
      command: command.name,
      namespace: command.namespace,
      visibility: command.visibility,
      mutates: command.mutates,
      effectful: command.effectful,
      required_roles: command.required_roles,
      stability: command.stability,
    },
  }));

  const reverseMap = Object.fromEntries(toolToCommand.entries());

  return {
    tools,
    reverseMap,
    catalogVersion: normalized.catalogVersion,
    filteredCount: filtered.length,
    totalCount: normalized.commands.length,
  };
}

function buildToolDescription(command) {
  const parts = [command.summary || command.name];
  const flags = [];
  if (command.mutates) flags.push("mutates durable state");
  if (command.effectful) flags.push("may trigger host effects (network/disk/clock/random)");
  if (flags.length > 0) {
    parts.push(`[${flags.join("; ")}]`);
  }
  parts.push(`(tier: ${command.visibility})`);
  return parts.join(" ");
}

function parseArgs(argv) {
  const options = {
    catalog: defaultCatalogPath,
    tier: "public",
    role: "owner",
    includeInner: false,
    describe: false,
    server: null,
    token: process.env.FORGE_SERVER_TOKEN ?? null,
    workspaceId: "agent-adapter",
    actor: "agent-adapter",
    json: true,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--catalog") options.catalog = argv[++index];
    else if (arg === "--tier") options.tier = argv[++index];
    else if (arg === "--role") options.role = argv[++index];
    else if (arg === "--include-inner") options.includeInner = true;
    else if (arg === "--describe") options.describe = true;
    else if (arg === "--server") options.server = argv[++index];
    else if (arg === "--token") options.token = argv[++index];
    else if (arg === "--workspace") options.workspaceId = argv[++index];
    else if (arg === "--actor") options.actor = argv[++index];
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown option: ${arg}`);
  }

  return options;
}

function printUsage() {
  console.log(`Usage: node tools/agent-adapter/catalog-to-tools.mjs [options]

Project the Terrane command catalog into LLM tool definitions.

Options:
  --catalog <path>     Catalog JSON (default: forge/data/commands.json)
  --describe           Fetch catalog via system.describe (local core-invoke)
  --server <url>       Fetch catalog via POST /bridge system.describe
  --token <token>      Bearer token for --server (or FORGE_SERVER_TOKEN)
  --tier <tier>        Max visibility tier: public|operator|admin|debug
  --role <role>        Agent role for filtering (default: owner)
  --workspace <id>     Workspace id for describe transport
  --actor <id>         Actor id for describe transport
  --include-inner      Include ctx.* inner-surface reference entries (default: off)
  --help               Show this message

Output JSON:
  { tools, reverseMap, catalogVersion, filteredCount, totalCount }
`);
}

async function cli(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    printUsage();
    return;
  }

  const catalog = await loadCatalog({
    catalogPath: options.catalog,
    describe: options.describe,
    server: options.server,
    token: options.token,
    tier: options.tier,
    role: options.role,
    workspaceId: options.workspaceId,
    actor: options.actor,
  });

  const result = catalogToTools(catalog, {
    tier: options.tier,
    role: options.role,
    includeInner: options.includeInner,
  });

  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  cli().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}