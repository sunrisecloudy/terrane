import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { examplesDir } from "../src/paths.js";
import { validatePackage, validateSourceSnippet } from "../src/package-validator.js";

test("all canonical example packages validate", () => {
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  assert.deepEqual(apps.sort(), ["api-dashboard", "core-replay-lab", "file-transformer", "notes-lite", "task-workbench"]);

  for (const app of apps) {
    const result = validatePackage(path.join(examplesDir, app));
    assert.equal(result.ok, true, `${app}: ${JSON.stringify(result.errors)}`);
    assert.equal(result.manifest.id, app);
  }
});

test("forbidden JS source snippets are rejected with policy codes", () => {
  const cases = [
    ["forbidden_network_api", "fetch('https://example.com')"],
    ["forbidden_eval", "eval('1 + 1')"],
    ["forbidden_storage_api", "localStorage.setItem('x', 'y')"],
    ["forbidden_native_bridge", "webkit.messageHandlers.bridge.postMessage({})"],
  ];

  for (const [code, source] of cases) {
    const result = validateSourceSnippet(source);
    assert.equal(result.ok, false);
    assert.equal(result.errors.some((error) => error.code === code), true, code);
  }
});

test("manifest.networkAllowlist is rejected", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.writeFileSync(path.join(dir, "index.html"), '<!doctype html><script src="app.js"></script>');
  fs.writeFileSync(path.join(dir, "styles.css"), "body { color: black; }");
  fs.writeFileSync(path.join(dir, "app.js"), "console.log('ok');");
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      id: "bad-app",
      name: "Bad App",
      version: "0.1.0",
      runtimeVersion: "0.1.0",
      dataVersion: 1,
      entry: "index.html",
      description: "Bad manifest",
      permissions: [],
      storagePrefix: "bad-app:",
      capabilities: { required: [], optional: [] },
      resourceBudget: {
        maxDomNodes: 10,
        maxStorageBytes: 10,
        maxBridgeCallsPerMinute: 10,
        maxNetworkRequestsPerMinute: 10,
        maxTimers: 10,
        maxLogLinesPerMinute: 10,
        maxPackageBytes: 100000,
        maxFileBytes: 100000,
      },
      networkPolicy: { allow: [] },
      networkAllowlist: [],
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "removed_manifest_field"), true);
});

test("interactive HTML elements must declare data-testid", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.writeFileSync(path.join(dir, "index.html"), '<!doctype html><button id="go">Go</button><script src="app.js"></script>');
  fs.writeFileSync(path.join(dir, "styles.css"), "body { color: black; }");
  fs.writeFileSync(path.join(dir, "app.js"), "console.log('ok');");
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      id: "bad-app",
      name: "Bad App",
      version: "0.1.0",
      runtimeVersion: "0.1.0",
      dataVersion: 1,
      entry: "index.html",
      description: "Bad HTML",
      permissions: [],
      storagePrefix: "bad-app:",
      capabilities: { required: [], optional: [] },
      resourceBudget: {
        maxDomNodes: 10,
        maxStorageBytes: 10,
        maxBridgeCallsPerMinute: 10,
        maxNetworkRequestsPerMinute: 10,
        maxTimers: 10,
        maxLogLinesPerMinute: 10,
        maxPackageBytes: 100000,
        maxFileBytes: 100000,
      },
      networkPolicy: { allow: [] },
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "missing_testid"), true);
});

test("dataVersion increases require consecutive migration files", () => {
  const dir = copyExamplePackage("notes-lite");
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.dataVersion = 2;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "migration_missing"), true);
});

test("migration steps cannot escape the app storage prefix", () => {
  const dir = copyExamplePackage("notes-lite");
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.dataVersion = 2;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  fs.mkdirSync(path.join(dir, "migrations"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "migrations", "1_to_2.json"),
    JSON.stringify({
      appId: "notes-lite",
      fromDataVersion: 1,
      toDataVersion: 2,
      steps: [{ op: "renameKey", from: "notes-lite:notes", to: "other-app:notes" }],
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "invalid_migration_prefix"), true);
});

function copyExamplePackage(name) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.cpSync(path.join(examplesDir, name), dir, { recursive: true });
  return dir;
}
