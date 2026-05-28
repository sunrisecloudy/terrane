import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { BridgeDispatcher } from "../src/bridge-dispatcher.js";
import { CoreEngine } from "../src/core.js";
import { examplesDir, repoRoot } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";
import { PlatformDatabase } from "../src/platform-database.js";
import { createPlatformKeypair, signPackage } from "../src/signing.js";

test("checked-in bridge fixtures match fake-host response codes", async () => {
  const db = new PlatformDatabase();
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const signed = signPackage({ manifest: pkg.manifest, files: pkg.files, keypair: createPlatformKeypair() });
  db.insertInstalledPackage({
    manifest: pkg.manifest,
    files: pkg.files,
    hashes: signed.hashes,
    validation: pkg.validation,
    signature: signed.signature,
    contentHashesDocument: signed.contentHashesDocument,
  });

  const dispatcher = new BridgeDispatcher({ database: db, core: new CoreEngine() });
  const sessionId = db.createRuntimeSession({ appId: "notes-lite" });
  const fixturesDir = path.join(repoRoot, "tests", "fixtures", "bridge");

  const cases = [
    ["valid-storage-get.json", true, null],
    ["invalid-storage-prefix.json", false, "permission_denied"],
    ["invalid-unknown-method.json", false, "unknown_method"],
  ];

  for (const [fileName, ok, code] of cases) {
    const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
    const response = await dispatcher.dispatch(fixture, { appId: fixture.appId, sessionId });
    assert.equal(response.ok, ok, fileName);
    if (code) {
      assert.equal(response.error.code, code, fileName);
    }
  }

  db.close();
});
