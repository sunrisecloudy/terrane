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

    const runtimeHtml = await fetch(`${started.url}/`).then((response) => response.text());
    assert.match(runtimeHtml, /Native AI Webapp Platform/);

    const examples = await fetch(`${started.url}/webapps/examples.json`).then((response) => response.json());
    assert.equal(examples.length, 5);
    assert.equal(examples.some((app) => app.id === "notes-lite"), true);

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

    const install = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: "Bearer test-token",
      },
      body: JSON.stringify({
        tool: "platform.install_webapp_package",
        args: { packagePath: path.join(examplesDir, "notes-lite") },
      }),
    }).then((response) => response.json());
    assert.equal(install.ok, true);
    assert.equal(install.result.appId, "notes-lite");

    const session = await fetch(`${started.url}/control/sessions`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: "Bearer test-token",
      },
      body: JSON.stringify({ appId: "notes-lite" }),
    }).then((response) => response.json());
    assert.equal(session.ok, true);
    assert.equal(session.result.appId, "notes-lite");
    assert.match(session.result.controlSessionId, /^control_/);

    const capabilities = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/capabilities`, {
      headers: { authorization: "Bearer test-token" },
    }).then((response) => response.json());
    assert.equal(capabilities.ok, true);
    assert.equal(capabilities.result.platform, "fake-host");

    const snapshot = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/snapshot`, {
      headers: { authorization: "Bearer test-token" },
    }).then((response) => response.json());
    assert.equal(snapshot.ok, true);
    assert.equal(snapshot.result.snapshot.appId, "notes-lite");

    const events = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/events`, {
      headers: { authorization: "Bearer test-token" },
    }).then((response) => response.json());
    assert.equal(events.ok, true);
    assert.equal(Array.isArray(events.result.bridgeCalls), true);

    const ended = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}`, {
      method: "DELETE",
      headers: { authorization: "Bearer test-token" },
    }).then((response) => response.json());
    assert.equal(ended.ok, true);
    assert.equal(ended.result.status, "ended");
  } finally {
    await started.close();
  }
});
