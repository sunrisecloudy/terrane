import { stdin, stdout } from "node:process";
import { ControlClient } from "./control-client.js";
import { TOOL_NAMES, toolDefinitions } from "./tool-contract.js";

const CONTROL_URL = process.env.PLATFORM_CONTROL_URL ?? "http://127.0.0.1:7878";
const CONTROL_TOKEN = process.env.PLATFORM_CONTROL_TOKEN ?? "dev-token-change-me";

export class McpStdioServer {
  constructor({ input = stdin, output = stdout, client = new ControlClient(CONTROL_URL, CONTROL_TOKEN) } = {}) {
    this.input = input;
    this.output = output;
    this.client = client;
    this.buffer = Buffer.alloc(0);
  }

  start() {
    this.input.on("data", (chunk) => this.receive(chunk));
  }

  async receive(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (true) {
      const parsed = takeMessage(this.buffer);
      if (!parsed) return;
      this.buffer = parsed.rest;
      await this.handle(parsed.message);
    }
  }

  async handle(message) {
    if (message.method?.startsWith("notifications/")) return;

    try {
      if (message.method === "initialize") {
        return this.reply(message.id, {
          protocolVersion: "2024-11-05",
          serverInfo: { name: "codex-platform-mcp", version: "0.1.0" },
          capabilities: { tools: {} },
        });
      }

      if (message.method === "ping") {
        return this.reply(message.id, {});
      }

      if (message.method === "tools/list") {
        return this.reply(message.id, { tools: toolDefinitions() });
      }

      if (message.method === "tools/call") {
        return this.callTool(message);
      }

      return this.error(message.id, -32601, `Unknown method: ${message.method}`);
    } catch (error) {
      return this.error(message.id, -32603, error instanceof Error ? error.message : String(error));
    }
  }

  async callTool(message) {
    const name = message.params?.name;
    if (!TOOL_NAMES.includes(name)) {
      return this.error(message.id, -32602, `Unknown tool: ${name}`);
    }

    const result = await this.client.command(name, message.params?.arguments ?? {});
    return this.reply(message.id, {
      content: [
        {
          type: "text",
          text: JSON.stringify(result, null, 2),
        },
      ],
      isError: result?.ok === false,
    });
  }

  reply(id, result) {
    this.write({ jsonrpc: "2.0", id, result });
  }

  error(id, code, message, data = {}) {
    this.write({ jsonrpc: "2.0", id, error: { code, message, data } });
  }

  write(message) {
    const body = JSON.stringify(message);
    this.output.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
  }
}

export function takeMessage(buffer) {
  const headerEnd = buffer.indexOf("\r\n\r\n");
  if (headerEnd === -1) return null;

  const header = buffer.subarray(0, headerEnd).toString("utf8");
  const lengthLine = header.split("\r\n").find((line) => line.toLowerCase().startsWith("content-length:"));
  if (!lengthLine) throw new Error("Missing Content-Length header");

  const length = Number(lengthLine.split(":")[1].trim());
  const bodyStart = headerEnd + 4;
  const bodyEnd = bodyStart + length;
  if (buffer.length < bodyEnd) return null;

  const body = buffer.subarray(bodyStart, bodyEnd).toString("utf8");
  return {
    message: JSON.parse(body),
    rest: buffer.subarray(bodyEnd),
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  new McpStdioServer().start();
}
