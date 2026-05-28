import { ControlClient } from "./control-client.js";
import { TOOL_NAMES, type ToolName } from "./tool-contract.js";

const CONTROL_URL = process.env.PLATFORM_CONTROL_URL ?? "http://127.0.0.1:29371";
const CONTROL_TOKEN = process.env.PLATFORM_CONTROL_TOKEN ?? "dev-token-change-me";

const client = new ControlClient(CONTROL_URL, CONTROL_TOKEN);

/**
 * Implementation note for Codex:
 * Replace this placeholder with the current @modelcontextprotocol/sdk server setup.
 * Each tool should call `client.command(toolName, args)` and return structured JSON.
 * Keep the mapping boring and mechanical.
 */
async function main() {
  console.error("codex-platform-mcp placeholder started");
  console.error(`Configured control URL: ${CONTROL_URL}`);
  console.error(`Tools to expose: ${TOOL_NAMES.join(", ")}`);

  // Pseudocode target:
  // const server = new McpServer({ name: "codex-platform-mcp", version: "0.1.0" });
  // for (const toolName of TOOL_NAMES) {
  //   server.tool(toolName, toolSchemaFor(toolName), async (args) => client.command(toolName, args));
  // }
  // await server.connect(new StdioServerTransport());

  // Keep process alive in placeholder mode so Codex sees startup diagnostics.
  await new Promise(() => {});
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
