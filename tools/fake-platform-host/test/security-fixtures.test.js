import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { validatePackage } from "../src/package-validator.js";
import { repoRoot } from "../src/paths.js";

const validatorCases = [
  ["uses-eval", "forbidden_eval"],
  ["uses-fetch", "forbidden_network_api"],
  ["uses-local-storage", "forbidden_storage_api"],
  ["remote-script", "forbidden_remote_script"],
  ["remote-css-import", "forbidden_css_import"],
  ["nested-iframe", "forbidden_embedded_context"],
  ["unknown-bridge-method", "forbidden_bridge_method"],
  ["parent-window-access", "forbidden_parent_access"],
  ["huge-package-size", "resource_budget_exceeded"],
];

test("malicious package fixtures are rejected with stable policy codes", () => {
  for (const [fixture, expectedCode] of validatorCases) {
    const result = validatePackage(maliciousPackagePath(fixture));
    assert.equal(result.ok, false, fixture);
    assert.equal(result.errors.some((error) => error.code === expectedCode), true, `${fixture}: ${JSON.stringify(result.errors)}`);
  }
});

test("runtime-denied malicious fixtures fail at the bridge boundary", async () => {
  await assertRuntimeDenied({
    fixture: "cross-app-storage",
    action: (host) => host.runControlCommand("runtime.storage_get", {
      appId: "cross-app-storage",
      key: "notes-lite:notes",
      defaultValue: [],
    }),
    expectedCode: "permission_denied",
  });

  await assertRuntimeDenied({
    fixture: "huge-storage-write",
    action: (host) => host.runControlCommand("runtime.storage_set", {
      appId: "huge-storage-write",
      key: "huge-storage-write:blob",
      value: "this value is deliberately too large",
    }),
    expectedCode: "resource_budget_exceeded",
  });

  await assertRuntimeDenied({
    fixture: "excessive-bridge-calls",
    action: async (host) => {
      const first = await host.runControlCommand("runtime.storage_get", {
        appId: "excessive-bridge-calls",
        key: "excessive-bridge-calls:first",
        defaultValue: null,
      });
      assert.equal(first.ok, true);
      return host.runControlCommand("runtime.storage_get", {
        appId: "excessive-bridge-calls",
        key: "excessive-bridge-calls:second",
        defaultValue: null,
      });
    },
    expectedCode: "resource_budget_exceeded",
  });
});

async function assertRuntimeDenied({ fixture, action, expectedCode }) {
  const packageDir = maliciousPackagePath(fixture);
  const validation = validatePackage(packageDir);
  assert.equal(validation.ok, true, `${fixture}: runtime-denied fixtures should pass static validation`);

  const host = new FakePlatformHost();
  try {
    const install = host.installPackage(packageDir);
    assert.equal(install.status, "enabled", fixture);
    const response = await action(host);
    assert.equal(response.ok, false, fixture);
    assert.equal(response.error.code, expectedCode, `${fixture}: ${JSON.stringify(response)}`);
  } finally {
    host.close();
  }
}

function maliciousPackagePath(fixture) {
  return path.join(repoRoot, "tests", "security", "malicious-packages", fixture);
}
