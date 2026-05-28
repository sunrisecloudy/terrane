import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { validatePackage } from "../src/package-validator.js";
import { repoRoot } from "../src/paths.js";

const cases = [
  ["uses-eval", "forbidden_eval"],
  ["uses-fetch", "forbidden_network_api"],
  ["uses-local-storage", "forbidden_storage_api"],
  ["remote-script", "forbidden_remote_script"],
  ["remote-css-import", "forbidden_css_import"],
  ["nested-iframe", "forbidden_embedded_context"],
  ["unknown-bridge-method", "forbidden_bridge_method"],
  ["parent-window-access", "forbidden_parent_access"],
];

test("malicious package fixtures are rejected with stable policy codes", () => {
  for (const [fixture, expectedCode] of cases) {
    const result = validatePackage(path.join(repoRoot, "tests", "security", "malicious-packages", fixture));
    assert.equal(result.ok, false, fixture);
    assert.equal(result.errors.some((error) => error.code === expectedCode), true, `${fixture}: ${JSON.stringify(result.errors)}`);
  }
});
