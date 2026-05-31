import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir, repoRoot } from "../src/paths.js";
import { validatePackage, validateSourceSnippet } from "../src/package-validator.js";

test("checked-in mutation fixtures fail with their declared error codes", async () => {
  const fixturesDir = path.join(repoRoot, "tests", "mutation");
  const files = fs.readdirSync(fixturesDir).filter((fileName) => fileName.endsWith(".json")).sort();

  for (const fileName of files) {
    const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
    assertMutationFixtureShape(fixture, fileName);
    const errorCodes = await runMutationFixture(fixture);
    assert.equal(
      errorCodes.includes(fixture.expectedError),
      true,
      `${fileName}: expected ${fixture.expectedError}, got ${JSON.stringify(errorCodes)}`,
    );
  }
});

function assertMutationFixtureShape(fixture, fileName) {
  assert.equal(typeof fixture.id, "string", fileName);
  assert.equal(typeof fixture.expectedError, "string", fileName);
}

async function runMutationFixture(fixture) {
  if (fixture.source) {
    return validateSourceSnippet(fixture.source).errors.map((error) => error.code);
  }
  if (fixture.html) {
    const packageDir = copyExample("notes-lite");
    fs.writeFileSync(
      path.join(packageDir, "index.html"),
      `<!doctype html><html><body><button data-testid="go-button">Go</button>${fixture.html}<script src="app.js"></script></body></html>`,
    );
    return validatePackage(packageDir).errors.map((error) => error.code);
  }
  if (fixture.files) {
    const packageDir = copyExample("notes-lite");
    for (const file of fixture.files) {
      fs.writeFileSync(path.join(packageDir, file.path), file.content);
    }
    return validatePackage(packageDir).errors.map((error) => error.code);
  }
  if (fixture.manifestPatch) {
    const packageDir = copyExample("notes-lite");
    const manifestPath = path.join(packageDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    fs.writeFileSync(manifestPath, JSON.stringify({ ...manifest, ...fixture.manifestPatch }, null, 2));
    return validatePackage(packageDir).errors.map((error) => error.code);
  }
  if (fixture.id === "missing-manifest-field") {
    const packageDir = copyExample("notes-lite");
    const manifestPath = path.join(packageDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    delete manifest.name;
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
    return validatePackage(packageDir).errors.map((error) => error.code);
  }
  if (fixture.id === "oversized-resource-budget") {
    const packageDir = copyExample("notes-lite");
    const manifestPath = path.join(packageDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.resourceBudget.maxPackageBytes = 1;
    manifest.resourceBudget.maxFileBytes = 1;
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
    return validatePackage(packageDir).errors.map((error) => error.code);
  }
  if (fixture.id === "post-signature-tamper") {
    const host = new ReferenceHost();
    try {
      const install = host.installPackage(path.join(examplesDir, "notes-lite"));
      host.database.run(
        "UPDATE app_files SET content_text = content_text || ? WHERE install_id = ? AND path = 'app.js'",
        "\n// tampered",
        install.installId,
      );
      try {
        await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
        return [];
      } catch (error) {
        return [error.code ?? "unknown"];
      }
    } finally {
      host.close();
    }
  }
  if (fixture.bridgeCall) {
    const appId = fixture.id === "invalid-network-origin" ? "api-dashboard" : "notes-lite";
    const host = new ReferenceHost();
    try {
      host.installPackage(path.join(examplesDir, appId));
      const response = await host.dispatchBridge(
        { id: `mutation_${fixture.id}`, method: fixture.bridgeCall.method, params: fixture.bridgeCall.params },
        { appId, sessionId: host.database.createRuntimeSession({ appId }) },
      );
      return response.ok ? [] : [response.error.code];
    } finally {
      host.close();
    }
  }
  throw new Error(`Unsupported mutation fixture: ${fixture.id}`);
}

function copyExample(name) {
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), `${name}-mutation-package-`));
  fs.cpSync(path.join(examplesDir, name), packageDir, { recursive: true });
  return packageDir;
}
