import { TOOL_NAMES } from "../src/tool-contract.js";
import { takeMessage } from "../src/server.js";

if (new Set(TOOL_NAMES).size !== TOOL_NAMES.length) {
  throw new Error("Tool names must be unique");
}

const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" });
const framed = Buffer.from(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
const parsed = takeMessage(framed);
if (!parsed || parsed.message.method !== "ping" || parsed.rest.length !== 0) {
  throw new Error("MCP frame parser check failed");
}

console.log(`codex-platform-mcp check ok (${TOOL_NAMES.length} tools)`);
