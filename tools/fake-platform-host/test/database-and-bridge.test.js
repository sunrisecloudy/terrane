import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { BridgeDispatcher } from "../src/bridge-dispatcher.js";
import { CoreEngine } from "../src/core.js";
import { examplesDir } from "../src/paths.js";
import { readPackage, packageHashes } from "../src/package-validator.js";
import { PlatformDatabase } from "../src/platform-database.js";

test("sqlite migrations apply and generated packages install transactionally", () => {
  const db = new PlatformDatabase();
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const hashes = packageHashes(pkg.manifest, pkg.files);
  const install = db.insertInstalledPackage({
    manifest: pkg.manifest,
    files: pkg.files,
    hashes,
    validation: pkg.validation,
  });

  assert.equal(install.appId, "notes-lite");
  assert.equal(db.activeInstall("notes-lite").installId, install.installId);
  assert.equal(db.queryAppVersions("notes-lite").length, 1);
  db.close();
});

test("bridge dispatch enforces permissions and storage prefixes", async () => {
  const db = new PlatformDatabase();
  for (const app of ["notes-lite", "task-workbench", "api-dashboard", "file-transformer"]) {
    const pkg = readPackage(path.join(examplesDir, app));
    db.insertInstalledPackage({
      manifest: pkg.manifest,
      files: pkg.files,
      hashes: packageHashes(pkg.manifest, pkg.files),
      validation: pkg.validation,
    });
  }
  const dispatcher = new BridgeDispatcher({ database: db, core: new CoreEngine() });

  const sessionId = db.createRuntimeSession({ appId: "notes-lite" });
  const set = await dispatcher.dispatch(
    { id: "req_set", method: "storage.set", params: { key: "notes-lite:notes", value: [{ title: "Hello" }] } },
    { appId: "notes-lite", sessionId },
  );
  assert.equal(set.ok, true);
  assert.equal(set.result.bytesWritten > 0, true);

  const get = await dispatcher.dispatch(
    { id: "req_get", method: "storage.get", params: { key: "notes-lite:notes", defaultValue: [] } },
    { appId: "notes-lite", sessionId },
  );
  assert.deepEqual(get.result.value, [{ title: "Hello" }]);

  const badPrefix = await dispatcher.dispatch(
    { id: "req_bad", method: "storage.get", params: { key: "task-workbench:tasks", defaultValue: [] } },
    { appId: "notes-lite", sessionId },
  );
  assert.equal(badPrefix.ok, false);
  assert.equal(badPrefix.error.code, "permission_denied");

  const unknown = await dispatcher.dispatch(
    { id: "req_unknown", method: "native.exec", params: { cmd: "ls" } },
    { appId: "notes-lite", sessionId },
  );
  assert.equal(unknown.ok, false);
  assert.equal(unknown.error.code, "unknown_method");

  const networkDenied = await dispatcher.dispatch(
    {
      id: "req_network",
      method: "network.request",
      params: { url: "https://api.example.com/status", method: "GET", headers: {}, body: null },
    },
    { appId: "notes-lite", sessionId },
  );
  assert.equal(networkDenied.ok, false);
  assert.equal(networkDenied.error.code, "permission_denied");

  const transform = await dispatcher.dispatch(
    {
      id: "req_core",
      method: "core.step",
      params: { app: "file-transformer", event: { type: "TransformText", payload: { text: "Hi", mode: "lowercase" } } },
    },
    { appId: "file-transformer", sessionId: db.createRuntimeSession({ appId: "file-transformer" }) },
  );
  assert.equal(transform.ok, true);
  assert.equal(transform.result.actions[0].text, "hi");

  assert.equal(db.queryBridgeCalls().length >= 6, true);
  db.close();
});
