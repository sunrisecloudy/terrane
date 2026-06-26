import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("Codex repair loop validates, installs, tests, patches, and retests an example app", async () => {
  const host = new ReferenceHost();
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), "notes-lite-repair-"));
  try {
    fs.cpSync(path.join(examplesDir, "notes-lite"), packageDir, { recursive: true });
    const appJsPath = path.join(packageDir, "app.js");
    const originalAppJs = fs.readFileSync(appJsPath, "utf8");
    fs.writeFileSync(appJsPath, originalAppJs.replaceAll("'storage.set'", "'storage.get'"));

    const repair = await host.runControlCommand("platform.run_repair_loop", {
      packagePath: packageDir,
      maxAttempts: 2,
      repairPatches: [{ path: "app.js", content: originalAppJs }],
    });

    assert.equal(repair.ok, true);
    assert.equal(repair.finalStatus, "passed");
    assert.equal(repair.attempts, 2);
    assert.deepEqual(repair.changedFiles, ["app.js"]);
    assert.equal(repair.testsRun.includes("smoke:notes-lite"), true);
    assert.equal(repair.attemptReports[0].status, "failed");
    assert.equal(
      repair.attemptReports[0].steps.some((step) => step.tool === "platform.apply_repair_patch" && step.status === "passed"),
      true,
    );
    assert.equal(repair.attemptReports[1].status, "passed");

    const active = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(active.appId, "notes-lite");
  } finally {
    host.close();
    fs.rmSync(packageDir, { recursive: true, force: true });
  }
});
