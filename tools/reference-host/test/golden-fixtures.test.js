import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { repoRoot } from "../src/paths.js";
import { validatePackage } from "../src/package-validator.js";

test("golden minimal-counter package installs and runs bundled smoke tests", async () => {
  const packageDir = materializeGoldenPackage("minimal-counter.package.json");
  const host = new ReferenceHost();
  try {
    const validation = validatePackage(packageDir);
    assert.equal(validation.ok, true, JSON.stringify(validation.errors));
    assert.equal(validation.manifest.id, "minimal-counter");

    const install = host.installPackage(packageDir);
    assert.equal(install.status, "enabled");
    assert.equal(install.smokeTest.status, "passed");
    assert.equal(install.smokeTest.total, 1);

    const run = await host.runControlCommand("runtime.run_smoke_tests", { appId: "minimal-counter" });
    assert.equal(run.status, "passed");
    assert.equal(run.result.total, 1);
    assert.equal(run.result.runner, "static");

    const runs = await host.runControlCommand("db.query_test_runs", { appId: "minimal-counter" });
    assert.equal(runs.some((row) => row.micro_test_id === "smoke:minimal-counter" && row.status === "passed"), true);
  } finally {
    host.close();
    fs.rmSync(packageDir, { recursive: true, force: true });
  }
});

function materializeGoldenPackage(fileName) {
  const goldenPackage = JSON.parse(fs.readFileSync(path.join(repoRoot, "tests", "golden", fileName), "utf8"));
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), "golden-package-"));
  for (const file of goldenPackage.files) {
    const destination = path.join(packageDir, file.path);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, file.path === "manifest.json" ? `${JSON.stringify(goldenPackage.manifest, null, 2)}\n` : file.content);
  }
  return packageDir;
}
