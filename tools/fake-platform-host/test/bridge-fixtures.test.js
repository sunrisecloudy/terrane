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
    ["valid-storage-set.json", true, null],
    ["valid-storage-list.json", true, null],
    ["valid-storage-remove.json", true, null],
    ["invalid-storage-prefix.json", false, "permission_denied"],
    ["invalid-permission-denied.json", false, "permission_denied"],
    ["invalid-unknown-method.json", false, "unknown_method"],
    ["valid-core-step.json", true, null],
    ["invalid-core-step-bad-json.json", true, null],
    ["valid-network-request-mocked.json", true, null],
    ["valid-network-policy-denied.json", false, "network_policy_denied"],
    ["valid-dialog-open-mocked.json", true, null],
    ["valid-dialog-cancelled.json", true, null],
    ["valid-runtime-capabilities.json", true, null],
    ["budget-exceeded-bridge-calls.json", false, "resource_budget_exceeded"],
    ["runtime-version-incompatible.json", false, "runtime_version_incompatible"],
  ];

  for (const [fileName, ok, code] of cases) {
    const db = new PlatformDatabase();
    try {
      const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
      assertBridgeFixtureShape(fixture, fileName);
      const pkg = readPackage(path.join(examplesDir, fixture.context.appId));
      const manifest = {
        ...pkg.manifest,
        ...(fixture.preconditions?.manifestPatch ?? {}),
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
      applyBridgeFixturePreconditions(db, fixture, sessionId);
      const { context, preconditions: _preconditions, expected: _expected, ...request } = fixture;
      const response = await dispatcher.dispatch(request, { appId: context.appId, sessionId });
      assert.equal(response.ok, fixture.expected?.ok ?? ok, fileName);
      const expectedCode = fixture.expected?.errorCode ?? code;
      if (expectedCode) {
        assert.equal(response.error.code, expectedCode, fileName);
      }
      if ("resultOk" in (fixture.expected ?? {})) {
        assert.equal(response.result?.ok, fixture.expected.resultOk, fileName);
      }
      if (fixture.expected?.resultErrorCode) {
        assert.equal(response.result?.error?.code, fixture.expected.resultErrorCode, fileName);
      }
    } finally {
      db.close();
    }
  }
});

function applyBridgeFixturePreconditions(db, fixture, sessionId) {
  const appId = fixture.context.appId;
  for (const mock of fixture.preconditions?.networkMocks ?? []) {
    db.addNetworkMock({
      sessionId,
      appId,
      method: mock.method ?? "GET",
      urlPattern: mock.urlPattern,
      response: mock.response,
    });
  }
  for (const mock of fixture.preconditions?.dialogMocks ?? []) {
    db.addDialogMock({
      sessionId,
      appId,
      dialogType: mock.dialogType,
      response: mock.response,
    });
  }
}

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
