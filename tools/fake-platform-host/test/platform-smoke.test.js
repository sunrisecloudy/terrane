import assert from "node:assert/strict";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";

test("checked-in platform smoke suite runs against fake-host", async () => {
  const host = new FakePlatformHost();
  try {
    const run = await host.runControlCommand("platform.run_platform_smoke", {
      smokePath: "tests/platform-smoke/all-example-apps.platform-smoke.json",
      platform: "fake-host",
    });

    assert.equal(run.status, "passed", JSON.stringify(run.result.failures));
    assert.equal(run.result.ok, true);
    assert.equal(run.result.totalApps, 5);
    assert.equal(run.result.apps.every((app) => app.ok), true);
    assert.equal(
      run.result.apps.every((app) => app.commands.some((command) => command.tool === "runtime.screenshot" && command.status === "passed")),
      true,
    );

    const persisted = await host.runControlCommand("db.query_test_runs", {});
    assert.equal(persisted.some((row) => row.micro_test_id === "platform-smoke:all-example-apps-cross-platform-smoke:fake-host"), true);
  } finally {
    host.close();
  }
});
