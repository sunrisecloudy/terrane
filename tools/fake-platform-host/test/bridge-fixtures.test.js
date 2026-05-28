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
  const fixturesDir = path.join(repoRoot, "tests", "fixtures", "bridge");

  const cases = [
    ["valid-storage-get.json", true, null],
    ["invalid-storage-prefix.json", false, "permission_denied"],
    ["invalid-unknown-method.json", false, "unknown_method"],
    ["budget-exceeded-bridge-calls.json", false, "resource_budget_exceeded"],
  ];

  for (const [fileName, ok, code] of cases) {
    const db = new PlatformDatabase();
    try {
      const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
      assertBridgeFixtureShape(fixture, fileName);
      const pkg = readPackage(path.join(examplesDir, "notes-lite"));
      const manifest = {
        ...pkg.manifest,
        resourceBudget: {
          ...pkg.manifest.resourceBudget,
          ...(fixture.preconditions?.resourceBudget ?? {}),
        },
      };
      const signed = signPackage({ manifest, files: pkg.files, keypair: createPlatformKeypair() });
      db.insertInstalledPackage({
        manifest,
        files: pkg.files,
        hashes: signed.hashes,
        validation: pkg.validation,
        signature: signed.signature,
        contentHashesDocument: signed.contentHashesDocument,
      });

      const dispatcher = new BridgeDispatcher({ database: db, core: new CoreEngine() });
      const sessionId = db.createRuntimeSession({ appId: fixture.context.appId });
      const { context, preconditions: _preconditions, expected: _expected, ...request } = fixture;
      const response = await dispatcher.dispatch(request, { appId: context.appId, sessionId });
      assert.equal(response.ok, fixture.expected?.ok ?? ok, fileName);
      const expectedCode = fixture.expected?.errorCode ?? code;
      if (expectedCode) {
        assert.equal(response.error.code, expectedCode, fileName);
      }
    } finally {
      db.close();
    }
  }
});

function assertBridgeFixtureShape(fixture, fileName) {
  assert.equal(typeof fixture.id, "string", fileName);
  assert.match(fixture.context.appId, /^[a-z][a-z0-9-]{2,63}$/, fileName);
  assert.equal(typeof fixture.method, "string", fileName);
  assert.equal(typeof fixture.params, "object", fileName);
  assert.equal(Array.isArray(fixture.params), false, fileName);
  if ("timestamp" in fixture) {
    assert.equal(Number.isInteger(fixture.timestamp), true, fileName);
  }
}
