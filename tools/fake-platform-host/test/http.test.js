import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
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
    assert.match(runtimeHtml, /__APP_RUNTIME_DEVTOOLS_ENABLED__/);

    const examples = await fetch(`${started.url}/webapps/examples.json`).then((response) => response.json());
    assert.equal(examples.length, 5);
    assert.equal(examples.some((app) => app.id === "notes-lite"), true);

    const unauthorized = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ tool: "platform.health", args: {} }),
    });
    assert.equal(unauthorized.status, 401);
    const unauthorizedBody = await unauthorized.json();
    assert.equal(unauthorizedBody.error.code, "control_auth_required");

    const validate = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({
        tool: "platform.validate_package",
        args: { packagePath: path.join(examplesDir, "notes-lite") },
      }),
    }).then((response) => response.json());
    assert.equal(validate.ok, true);
    assert.equal(validate.result.ok, true);

    const directValidate = await fetch(`${started.url}/packages/validate`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({ packagePath: path.join(examplesDir, "notes-lite") }),
    }).then((response) => response.json());
    assert.equal(directValidate.ok, true);
    assert.equal(directValidate.result.ok, true);

    const missingMountToken = await fetch(`${started.url}/bridge`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-app-id": "notes-lite",
      },
      body: JSON.stringify({ id: "req_missing_mount", method: "runtime.capabilities", params: {} }),
    }).then((response) => response.json());
    assert.equal(missingMountToken.ok, false);
    assert.equal(missingMountToken.error.code, "bridge.unauthorized_channel");

    const bridgeCapabilities = await fetch(`${started.url}/bridge`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-app-id": "notes-lite",
        "x-mount-token": "test-mount-token",
      },
      body: JSON.stringify({ id: "req_caps", method: "runtime.capabilities", params: {} }),
    }).then((response) => response.json());
    assert.equal(bridgeCapabilities.ok, true);
    assert.equal(bridgeCapabilities.result.platform, "fake");
    assert.equal(bridgeCapabilities.result.target, "fake-host");

    const install = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({
        tool: "platform.install_webapp_package",
        args: { packagePath: path.join(examplesDir, "notes-lite") },
      }),
    }).then((response) => response.json());
    assert.equal(install.ok, true);
    assert.equal(install.result.appId, "notes-lite");

    const versions = await fetch(`${started.url}/apps/notes-lite/versions`, {
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(versions.ok, true);
    assert.equal(versions.result.some((version) => version.appId === "notes-lite"), true);

    const report = await fetch(`${started.url}/apps/notes-lite/install-report`, {
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(report.ok, true);
    assert.equal(report.result.appId, "notes-lite");

    const session = await fetch(`${started.url}/control/sessions`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({ appId: "notes-lite" }),
    }).then((response) => response.json());
    assert.equal(session.ok, true);
    assert.equal(session.result.appId, "notes-lite");
    assert.match(session.result.controlSessionId, /^control_/);

    const capabilities = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/capabilities`, {
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(capabilities.ok, true);
    assert.equal(capabilities.result.platform, "fake");
    assert.equal(capabilities.result.target, "fake-host");

    const snapshot = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/snapshot`, {
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(snapshot.ok, true);
    assert.equal(snapshot.result.snapshot.appId, "notes-lite");

    const events = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}/events`, {
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(events.ok, true);
    assert.equal(Array.isArray(events.result.bridgeCalls), true);

    const ended = await fetch(`${started.url}/control/sessions/${session.result.controlSessionId}`, {
      method: "DELETE",
      headers: { "x-platform-control-token": "test-token" },
    }).then((response) => response.json());
    assert.equal(ended.ok, true);
    assert.equal(ended.result.status, "ended");

    const dbSnapshot = await fetch(`${started.url}/db/snapshot`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({}),
    }).then((response) => response.json());
    assert.equal(dbSnapshot.ok, true);
    assert.equal(Array.isArray(dbSnapshot.result.apps), true);

    const auditRows = started.host.database.queryControlCommands();
    assert.equal(
      auditRows.some((row) => row.path === "/control/command" && row.decision === "rejected" && row.error_code === "control_auth_required"),
      true,
    );
    assert.equal(
      auditRows.some((row) => row.tool === "platform.validate_package" && row.path === "/control/command" && row.decision === "accepted"),
      true,
    );
    assert.equal(auditRows.some((row) => row.tool === "db.snapshot" && row.path === "/db/snapshot" && row.decision === "accepted"), true);
  } finally {
    await started.close();
  }
});

test("fake host writes a per-launch control token file", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-token-"));
  const tokenFile = path.join(dir, "control.token");
  const started = await startFakePlatformHost({ port: 0, tokenFile });
  try {
    const token = fs.readFileSync(tokenFile, "utf8").trim();
    assert.equal(token, started.controlToken);
    assert.match(token, /^[A-Za-z0-9_-]{43}$/);

    const health = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": token,
      },
      body: JSON.stringify({ tool: "platform.health", args: {} }),
    }).then((response) => response.json());
    assert.equal(health.ok, true);
  } finally {
    await started.close();
  }
});

test("seeded bundled apps can use bridge permissions through HTTP", async () => {
  const started = await startFakePlatformHost({ port: 0, controlToken: "test-token", seedBundled: true });
  try {
    const response = await fetch(`${started.url}/bridge`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-app-id": "notes-lite",
        "x-mount-token": "test-mount-token",
      },
      body: JSON.stringify({
        id: "req_storage",
        method: "storage.get",
        params: { key: "notes-lite:notes", defaultValue: [] },
      }),
    }).then((result) => result.json());

    assert.equal(response.ok, true);
    assert.deepEqual(response.result.value, []);
  } finally {
    await started.close();
  }
});

test("repeated control auth failures trigger a temporary ban and audit row", async () => {
  const started = await startFakePlatformHost({ port: 0, controlToken: "test-token" });
  try {
    const request = () =>
      fetch(`${started.url}/control/command`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ tool: "platform.health", args: {} }),
      });

    assert.equal((await request()).status, 401);
    assert.equal((await request()).status, 401);
    const banned = await request();
    assert.equal(banned.status, 403);
    assert.equal((await banned.json()).error.code, "control_connection_banned");

    const stillBanned = await fetch(`${started.url}/control/command`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-platform-control-token": "test-token",
      },
      body: JSON.stringify({ tool: "platform.health", args: {} }),
    });
    assert.equal(stillBanned.status, 403);

    const auditRows = started.host.database.queryControlCommands();
    assert.equal(
      auditRows.some((row) => row.path === "/control/command" && row.decision === "rejected" && row.error_code === "control_connection_banned"),
      true,
    );
  } finally {
    await started.close();
  }
});
