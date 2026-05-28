import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir, repoRoot } from "../src/paths.js";

test("all example apps pass fake-host static accessibility audit", async () => {
  const host = new FakePlatformHost();
  try {
    const apps = exampleApps();
    for (const app of apps) {
      host.installPackage(path.join(examplesDir, app));
      const report = await host.runControlCommand("runtime.run_accessibility_audit", { appId: app });
      assert.equal(report.status, "pass", `${app}: ${JSON.stringify(report.checks)}`);

      const snapshot = await host.runControlCommand("runtime.accessibility_snapshot", { appId: app });
      assert.equal(snapshot.landmarks.some((landmark) => landmark.role === "main"), true, app);
      assert.equal(snapshot.headings.some((heading) => heading.level === 1), true, app);
      assert.equal(snapshot.controls.every((control) => control.name.length > 0), true, app);

      const assertion = await host.runControlCommand("runtime.assert_accessibility", {
        appId: app,
        rule: "no_unlabeled_controls",
      });
      assert.equal(assertion.ok, true, app);
    }
  } finally {
    host.close();
  }
});

test("checked-in accessibility microtests execute against fake-host controls", async () => {
  const fixturesDir = path.join(repoRoot, "tests", "accessibility");
  const files = fs.readdirSync(fixturesDir).filter((fileName) => fileName.endsWith(".json")).sort();

  for (const fileName of files) {
    const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
    for (const appId of fixture.targetApps ?? []) {
      const host = new FakePlatformHost();
      try {
        host.installPackage(path.join(examplesDir, appId));
        for (const step of fixture.steps ?? []) {
          const result = await host.runControlCommand(step.tool, { ...(step.args ?? {}), appId });
          assert.equal(result.ok ?? true, true, `${fileName}:${appId}:${step.tool}`);
        }
      } finally {
        host.close();
      }
    }
  }
});

function exampleApps() {
  return fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory()).sort();
}
