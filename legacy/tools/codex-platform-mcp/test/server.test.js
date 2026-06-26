import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import test from "node:test";
import { McpStdioServer, takeMessage } from "../src/server.js";

function frame(message) {
  const body = JSON.stringify(message);
  return Buffer.from(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
}

function parseFrame(buffer) {
  return takeMessage(Buffer.from(buffer)).message;
}

test("frame parser handles one complete message", () => {
  const message = { jsonrpc: "2.0", id: 1, method: "ping" };
  const parsed = takeMessage(frame(message));
  assert.deepEqual(parsed.message, message);
  assert.equal(parsed.rest.length, 0);
});

test("MCP server lists tools and forwards tool calls", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const writes = [];
  output.on("data", (chunk) => writes.push(chunk));
  const client = {
    async command(tool, args) {
      return { ok: true, result: { tool, args } };
    },
  };

  const server = new McpStdioServer({ input, output, client });

  await server.receive(frame({ jsonrpc: "2.0", id: 1, method: "tools/list" }));
  const listResponse = parseFrame(Buffer.concat(writes.splice(0)));
  assert.equal(listResponse.result.tools.some((tool) => tool.name === "platform.health"), true);

  await server.receive(
    frame({
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: { name: "platform.health", arguments: { target: "reference-host" } },
    }),
  );
  const callResponse = parseFrame(Buffer.concat(writes.splice(0)));
  assert.equal(callResponse.result.isError, false);
  assert.match(callResponse.result.content[0].text, /platform.health/);
});

test("MCP server rejects invalid tool arguments before forwarding", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const writes = [];
  output.on("data", (chunk) => writes.push(chunk));
  let calls = 0;
  const client = {
    async command() {
      calls += 1;
      return { ok: true };
    },
  };

  const server = new McpStdioServer({ input, output, client });
  await server.receive(
    frame({
      jsonrpc: "2.0",
      id: 3,
      method: "tools/call",
      params: { name: "platform.open_webapp", arguments: {} },
    }),
  );

  const response = parseFrame(Buffer.concat(writes.splice(0)));
  assert.equal(response.error.code, -32602);
  assert.match(response.error.message, /missing required argument: appId/);
  assert.equal(calls, 0);
});
