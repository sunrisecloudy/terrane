import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { examplesDir } from "../src/paths.js";
import { startFakePlatformHost } from "../src/server.js";

test("http health and token-protected control command work", async () => {
  const started = await startFakePlatformHost({ port: 0, controlToken: "test-token" });
  try {
    const health = await fetch(`${started.url}/health`).then((response) => response.json());
    assert.equal(health.ok, true);
    assert.equal(health.db, "sqlite-mem");

    const unauthorized = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ tool: "platform.health", args: {} }),
    });
    assert.equal(unauthorized.status, 401);

    const validate = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: "Bearer test-token",
      },
      body: JSON.stringify({
        tool: "platform.validate_package",
        args: { packagePath: path.join(examplesDir, "notes-lite") },
      }),
    }).then((response) => response.json());
    assert.equal(validate.ok, true);
    assert.equal(validate.result.ok, true);
  } finally {
    await started.close();
  }
});
