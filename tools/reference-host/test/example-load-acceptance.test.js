import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

const exampleApps = [
  ["notes-lite", "notes-title"],
  ["task-workbench", "task-workbench-title"],
  ["file-transformer", "file-transformer-title"],
  ["api-dashboard", "api-dashboard-title"],
  ["core-replay-lab", "core-replay-title"],
  ["calendar-planner", "calendar-planner-title"],
];

test("all bundled example apps validate, install, open, snapshot, and smoke-test", async () => {
  const host = new ReferenceHost();
  try {
    for (const [appId, titleTestId] of exampleApps) {
      const packagePath = path.join(examplesDir, appId);

      const validation = await host.runControlCommand("platform.validate_package", { packagePath });
      assert.equal(validation.ok, true, `${appId}: package validates`);

      const install = await host.runControlCommand("platform.install_webapp_package", { packagePath });
      assert.equal(install.status, "enabled", `${appId}: package installs and enables`);
      assert.equal(install.smokeTest.status, "passed", `${appId}: bundled smoke test passes during install`);

      const opened = await host.runControlCommand("platform.open_webapp", { appId });
      assert.equal(opened.appId, appId, `${appId}: runtime session opens`);
      assert.equal(typeof opened.sessionId, "string", `${appId}: runtime session id is returned`);

      const visible = await host.runControlCommand("runtime.assert_visible", { appId, testId: titleTestId });
      assert.equal(visible.ok, true, `${appId}: title test id is visible`);

      const snapshot = await host.runControlCommand("runtime.snapshot", { appId });
      assert.equal(snapshot.appId, appId, `${appId}: snapshot uses the opened app`);
      assert.equal(snapshot.testIds.includes(titleTestId), true, `${appId}: snapshot includes title test id`);

      const smoke = await host.runControlCommand("runtime.run_smoke_tests", { appId });
      assert.equal(smoke.status, "passed", `${appId}: smoke test runner passes`);
      assert.equal(smoke.result.total > 0, true, `${appId}: smoke test has assertions`);
    }
  } finally {
    host.close();
  }
});
