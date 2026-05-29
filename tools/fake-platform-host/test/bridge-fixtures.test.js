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

const contractPlatforms = ["fake-host", "macos", "ios-simulator", "android-emulator", "windows", "linux", "server"];

test("checked-in bridge fixtures match fake-host expected responses", async () => {
  const fixturesDir = path.join(repoRoot, "tests", "fixtures", "bridge");
  const required = new Set([
    "valid-storage-get.json",
    "valid-storage-set.json",
    "valid-storage-list.json",
    "valid-storage-remove.json",
    "invalid-unknown-method.json",
    "invalid-permission-denied.json",
    "invalid-storage-prefix.json",
    "valid-core-step.json",
    "invalid-core-step-bad-json.json",
    "valid-network-request-mocked.json",
    "valid-network-policy-denied.json",
    "valid-dialog-open-mocked.json",
    "valid-dialog-cancelled.json",
    "valid-dialog-save-mocked.json",
    "valid-app-log.json",
    "valid-runtime-capabilities.json",
    "budget-exceeded-bridge-calls.json",
    "runtime-version-incompatible.json",
  ]);

  const files = fs.readdirSync(fixturesDir).filter((fileName) => fileName.endsWith(".json")).sort();
  const missing = [...required].filter((fileName) => !files.includes(fileName));
  assert.deepEqual(missing, [], "docs/08 required bridge fixtures must be checked in");

  for (const fileName of files) {
    const db = new PlatformDatabase();
    try {
      const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
      assertBridgeFixtureShape(fixture, fileName);
      const expected = expectedForPlatform(fixture, "fake-host");
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
      const {
        context,
        preconditions: _preconditions,
        expected: _expected,
        expectedByPlatform: _expectedByPlatform,
        platforms: _platforms,
        ...request
      } = fixture;
      const response = await dispatcher.dispatch(request, { appId: context.appId, sessionId });
      assert.equal(response.ok, expected?.ok, fileName);
      if (expected?.errorCode) {
        assert.equal(response.error.code, expected.errorCode, fileName);
      }
      if ("resultOk" in (expected ?? {})) {
        assert.equal(response.result?.ok, expected.resultOk, fileName);
      }
      if (expected?.resultErrorCode) {
        assert.equal(response.result?.error?.code, expected.resultErrorCode, fileName);
      }
      if (expected?.resultSubset) {
        assertDeepSubset(response.result, expected.resultSubset, `${fileName} result`);
      }
      if (expected?.errorDetailsSubset) {
        assertDeepSubset(response.error?.details, expected.errorDetailsSubset, `${fileName} error details`);
      }
    } finally {
      db.close();
    }
  }
});

function expectedForPlatform(fixture, platform) {
  return fixture.expectedByPlatform?.[platform] ?? fixture.expected;
}

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

function assertDeepSubset(actual, expected, label) {
  if (Array.isArray(expected)) {
    assert.deepEqual(actual, expected, label);
    return;
  }
  if (expected && typeof expected === "object") {
    assert.equal(Boolean(actual && typeof actual === "object" && !Array.isArray(actual)), true, label);
    for (const [key, value] of Object.entries(expected)) {
      assertDeepSubset(actual[key], value, `${label}.${key}`);
    }
    return;
  }
  assert.deepEqual(actual, expected, label);
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
  assert.deepEqual(fixture.platforms, contractPlatforms, `${fileName} platforms`);
}
