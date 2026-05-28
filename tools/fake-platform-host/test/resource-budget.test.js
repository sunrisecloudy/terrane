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
    assert.equal(second.error.details.limit, 1);
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
  const manifestPath = path.join(packageDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.resourceBudget = { ...manifest.resourceBudget, ...patch };
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
}
