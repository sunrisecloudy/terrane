#!/usr/bin/env node
import { fileURLToPath } from "node:url";
import {
  commandVisibleTo,
  filterCatalog,
  findCommand,
  isInnerSurface,
  loadCatalog,
  normalizeCatalog,
} from "./lib/catalog.mjs";
import { buildNameMaps, toolNameToCommand } from "./lib/names.mjs";
import { resolveInputSchema, validatePayload } from "./lib/schema.mjs";
import { buildEnvelope, invokeCoreCommand } from "./lib/transport.mjs";
import { defaultCatalogPath } from "./lib/paths.mjs";

/**
 * Execute one LLM tool call against Terrane core transport.
 */
export async function executeTool(toolName, args = {}, options = {}) {
  const catalog = await loadCatalogForExecution(options);
  const filtered = filterCatalog(catalog.commands, {
    tier: options.tier ?? "public",
    role: options.role ?? "owner",
    includeInner: false,
  });
  const { toolToCommand } = buildNameMaps(filtered);

  const commandName = toolNameToCommand(toolName, toolToCommand);
  if (!commandName) {
    return {
      ok: false,
      error: {
        code: "tool_not_offered",
        message: `tool ${toolName} is not offered for tier=${options.tier ?? "public"} role=${options.role ?? "owner"}`,
      },
    };
  }

  const descriptor = findCommand(catalog.commands, commandName);
  if (!descriptor) {
    return {
      ok: false,
      error: {
        code: "command_missing",
        message: `catalog is missing descriptor for ${commandName}`,
      },
    };
  }

  if (isInnerSurface(descriptor)) {
    return {
      ok: false,
      error: {
        code: "inner_surface_refused",
        message: `${commandName} is an inner ctx.* reference entry and cannot be invoked by an agent`,
      },
    };
  }

  if (!commandVisibleTo(descriptor, { tier: options.tier ?? "public", role: options.role ?? "owner" })) {
    return {
      ok: false,
      error: {
        code: "tier_or_role_denied",
        message: `${commandName} is outside the agent tier/role ceiling`,
      },
    };
  }

  const inputSchema = resolveInputSchema(descriptor);
  const validation = validatePayload(args, inputSchema, { commandName });
  if (!validation.ok) {
    return {
      ok: false,
      error: {
        code: "payload_validation",
        message: validation.error,
        details: validation.details ?? null,
      },
    };
  }

  const needsConfirm = descriptor.mutates || descriptor.effectful;
  const confirmed = Boolean(options.confirm) || args.confirm === true;
  if (needsConfirm && !confirmed && !options.dryRun) {
    return {
      ok: false,
      error: {
        code: "confirm_required",
        message: `${commandName} is ${descriptor.mutates ? "mutating" : ""}${descriptor.mutates && descriptor.effectful ? " and " : ""}${descriptor.effectful ? "effectful" : ""}; pass --confirm or include "confirm": true in tool args`,
      },
      descriptor: {
        name: commandName,
        mutates: descriptor.mutates,
        effectful: descriptor.effectful,
      },
    };
  }

  const envelope = buildEnvelope({
    name: commandName,
    payload: sanitizePayload(args),
    requestId: options.requestId ?? `agent-${toolName}`,
    workspaceId: options.workspaceId ?? "agent-adapter",
    actor: options.actor ?? "agent-adapter",
    role: options.role ?? "owner",
  });

  if (options.dryRun) {
    return {
      ok: true,
      dry_run: true,
      envelope,
      command: commandName,
      tool: toolName,
    };
  }

  const response = await invokeCoreCommand(envelope, {
    server: options.server ?? null,
    token: options.token ?? null,
  });

  return {
    ok: response.ok,
    response,
    command: commandName,
    tool: toolName,
  };
}

function sanitizePayload(args) {
  if (!args || typeof args !== "object" || Array.isArray(args)) {
    return {};
  }
  const payload = { ...args };
  delete payload.confirm;
  return payload;
}

async function loadCatalogForExecution(options) {
  if (options.catalogDocument) {
    return normalizeCatalog(options.catalogDocument);
  }

  if (options.toolsBundle) {
    return normalizeCatalog(options.toolsBundle);
  }

  return loadCatalog({
    catalogPath: options.catalog ?? defaultCatalogPath,
    describe: options.describe ?? false,
    server: options.server ?? null,
    token: options.token ?? null,
    tier: options.tier ?? "public",
    role: options.role ?? "owner",
    workspaceId: options.workspaceId ?? "agent-adapter",
    actor: options.actor ?? "agent-adapter",
  });
}

function parseArgs(argv) {
  const options = {
    tool: null,
    args: {},
    catalog: defaultCatalogPath,
    tier: "public",
    role: "owner",
    describe: false,
    server: null,
    token: process.env.FORGE_SERVER_TOKEN ?? null,
    workspaceId: "agent-adapter",
    actor: "agent-adapter",
    requestId: null,
    confirm: false,
    dryRun: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--tool") options.tool = argv[++index];
    else if (arg === "--args") options.args = JSON.parse(argv[++index]);
    else if (arg === "--catalog") options.catalog = argv[++index];
    else if (arg === "--tier") options.tier = argv[++index];
    else if (arg === "--role") options.role = argv[++index];
    else if (arg === "--describe") options.describe = true;
    else if (arg === "--server") options.server = argv[++index];
    else if (arg === "--token") options.token = argv[++index];
    else if (arg === "--workspace") options.workspaceId = argv[++index];
    else if (arg === "--actor") options.actor = argv[++index];
    else if (arg === "--request-id") options.requestId = argv[++index];
    else if (arg === "--confirm") options.confirm = true;
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown option: ${arg}`);
  }

  if (!options.tool) {
    throw new Error("--tool is required");
  }

  return options;
}

function printUsage() {
  console.log(`Usage: node tools/agent-adapter/execute-tool.mjs --tool <name> [options]

Execute one projected tool call via core-invoke or POST /bridge.

Options:
  --tool <name>        Tool name (e.g. query_execute)
  --args <json>        Tool arguments object (default: {})
  --catalog <path>     Catalog JSON (default: forge/data/commands.json)
  --describe           Load catalog via system.describe before execution
  --server <url>       Remote core via POST /bridge
  --token <token>      Bearer token for --server
  --tier <tier>        Agent tier ceiling (re-checked at execution)
  --role <role>        Agent role (default: owner)
  --workspace <id>     Workspace id in CoreCommand envelope
  --actor <id>         Actor id in CoreCommand envelope
  --request-id <id>    Optional request_id override
  --confirm            Allow mutating/effectful commands
  --dry-run            Validate + print envelope without executing
  --help               Show this message
`);
}

async function cli(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    printUsage();
    return;
  }

  const result = await executeTool(options.tool, options.args, options);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  if (!result.ok) {
    process.exit(1);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  cli().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}

