import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("storage.set rejects writes over manifest maxStorageBytes", async () => {
  const host = new FakePlatformHost();
  const packageDir = copyExample("notes-lite");
  try {
    updateManifestBudget(packageDir, { maxStorageBytes: 8 });
    host.installPackage(packageDir);

    const response = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "this is too large" }],
    });

    assert.equal(response.ok, false);
    assert.equal(response.error.code, "resource_budget_exceeded");
    assert.equal(response.error.details.budget, "maxStorageBytes");
    assert.equal(response.error.details.current > 8, true);
    assert.equal(response.error.details.max, 8);
    assert.equal(response.error.details.limit, 8);
  } finally {
    host.close();
  }
});

test("bridge call budget rejects calls after the per-minute limit", async () => {
  const host = new FakePlatformHost();
  const packageDir = copyExample("notes-lite");
  try {
    updateManifestBudget(packageDir, { maxBridgeCallsPerMinute: 1 });
    host.installPackage(packageDir);

    const first = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(first.ok, true);

    const second = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(second.ok, false);
    assert.equal(second.error.code, "resource_budget_exceeded");
    assert.equal(second.error.details.budget, "maxBridgeCallsPerMinute");
    assert.equal(second.error.details.current, 2);
    assert.equal(second.error.details.max, 1);
    assert.equal(second.error.details.limit, 1);
  } finally {
    host.close();
  }
});

test("repeated budget violations quarantine active version and restore previous install", async () => {
  const host = new FakePlatformHost();
  const firstDir = copyExample("notes-lite");
  const secondDir = copyExample("notes-lite");
  try {
    updateManifestBudget(firstDir, { maxBridgeCallsPerMinute: 0 });
    const first = host.installPackage(firstDir);

    updateManifest(secondDir, (manifest) => {
      manifest.version = "0.2.0";
      manifest.description = "Budget rollback test version.";
      manifest.resourceBudget = { ...manifest.resourceBudget, maxBridgeCallsPerMinute: 0 };
    });
    const second = host.installPackage(secondDir);
    assert.equal(host.database.activeInstallId("notes-lite"), second.installId);

    for (let attempt = 0; attempt < 3; attempt += 1) {
      const response = await host.runControlCommand("runtime.storage_get", {
        appId: "notes-lite",
        key: "notes-lite:notes",
        defaultValue: [],
      });
      assert.equal(response.ok, false);
      assert.equal(response.error.code, "resource_budget_exceeded");
    }

    const versions = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    assert.equal(versions.find((version) => version.installId === second.installId).status, "quarantined");
    assert.equal(versions.find((version) => version.installId === first.installId).status, "enabled");
    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");

    const events = host.database.all(
      "SELECT action, install_id, previous_install_id, details_json FROM app_installations WHERE app_id = ? ORDER BY created_at",
      "notes-lite",
    );
    const quarantine = events.find((event) => event.action === "quarantine" && event.install_id === second.installId);
    assert.ok(quarantine);
    assert.equal(quarantine.previous_install_id, first.installId);
    assert.equal(JSON.parse(quarantine.details_json).reason, "resource_budget_exceeded");
    const rollback = events.find((event) => event.action === "rollback" && event.install_id === first.installId);
    assert.ok(rollback);
    assert.equal(rollback.previous_install_id, second.installId);
  } finally {
    host.close();
  }
});

test("runtime sessions persist resource high-water marks", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    const session = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });

    const response = await host.runControlCommand("runtime.storage_set", {
      sessionId: session.sessionId,
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "High water" }],
    });
    assert.equal(response.ok, true);

    const row = host.database.snapshot().runtime_sessions.find((candidate) => candidate.session_id === session.sessionId);
    assert.ok(row);
    const highWater = JSON.parse(row.resource_high_water_json);
    assert.equal(highWater.appId, "notes-lite");
    assert.equal(highWater.storageBytes > 0, true);
    assert.equal(highWater.bridgeCallsLastMinute >= 1, true);
  } finally {
    host.close();
  }
});

function copyExample(name) {
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), `${name}-budget-package-`));
  fs.cpSync(path.join(examplesDir, name), packageDir, { recursive: true });
  return packageDir;
}

function updateManifestBudget(packageDir, patch) {
  updateManifest(packageDir, (manifest) => {
    manifest.resourceBudget = { ...manifest.resourceBudget, ...patch };
  });
}

function updateManifest(packageDir, patcher) {
  const manifestPath = path.join(packageDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  patcher(manifest);
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
}
