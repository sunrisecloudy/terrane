import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const forgeDir = path.join(repoRoot, "forge");

test(
  "Forge server exposes the CoreCommand HTTP bridge contract",
  { timeout: 120_000 },
  async (t) => {
    const port = await freePort();
    const workspaceId = "reference-host-contract";
    const started = startForgeServer(port, workspaceId);
    t.after(async () => {
      await stopForgeServer(started);
    });

    const baseUrl = `http://127.0.0.1:${port}`;
    const health = await waitForJson(`${baseUrl}/health`, started);
    assert.equal(health.status, 200);
    assert.equal(health.body.ok, true);
    assert.equal(health.body.service, "forge-server");

    const opened = await postJson(`${baseUrl}/bridge`, coreCommand("workspace.open", workspaceId));
    assert.equal(opened.status, 200);
    assert.equal(opened.body.ok, true);
    assert.equal(opened.body.payload.workspace_id, workspaceId);

    const malformed = await fetch(`${baseUrl}/bridge`, {
      method: "POST",
      body: "{",
    });
    assert.equal(malformed.status, 400);
    const malformedBody = await malformed.json();
    assert.equal(malformedBody.ok, false);
    assert.equal(malformedBody.error.kind, "ValidationError");

    const drained = await fetch(`${baseUrl}/events/drain`, { method: "POST" });
    assert.equal(drained.status, 200);
    const drainedBody = await drained.json();
    assert.equal(drainedBody.ok, true);
    assert.equal(Array.isArray(drainedBody.events), true);

    const missing = await fetch(`${baseUrl}/control/command`, { method: "POST", body: "{}" });
    assert.equal(missing.status, 404);
  },
);

function coreCommand(name, workspaceId, payload = {}) {
  return {
    request_id: `ref-${name}`,
    actor: { actor: "dev", role: "owner" },
    workspace_id: workspaceId,
    applet_id: null,
    name,
    payload,
  };
}

function startForgeServer(port, workspaceId) {
  const child = spawn(
    "cargo",
    ["run", "--quiet", "-p", "forge-server", "--", "--bind", `127.0.0.1:${port}`, "--workspace-id", workspaceId],
    {
      cwd: forgeDir,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const output = [];
  child.stdout.on("data", (chunk) => output.push(String(chunk)));
  child.stderr.on("data", (chunk) => output.push(String(chunk)));
  return { child, output };
}

async function stopForgeServer(started) {
  if (started.child.exitCode != null) return;
  started.child.kill("SIGTERM");
  await new Promise((resolve) => {
    const timeout = setTimeout(resolve, 1000);
    started.child.once("exit", () => {
      clearTimeout(timeout);
      resolve();
    });
  });
}

async function waitForJson(url, started) {
  let lastError;
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    if (started.child.exitCode != null) {
      throw new Error(`forge-server exited early:\n${started.output.join("")}`);
    }
    try {
      const response = await fetch(url);
      return { status: response.status, body: await response.json() };
    } catch (error) {
      lastError = error;
      await delay(100);
    }
  }
  throw new Error(`forge-server did not become ready: ${lastError?.message ?? "unknown"}\n${started.output.join("")}`);
}

async function postJson(url, value) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
  });
  return { status: response.status, body: await response.json() };
}

async function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => {
        if (address && typeof address === "object") {
          resolve(address.port);
        } else {
          reject(new Error("failed to allocate a free port"));
        }
      });
    });
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
