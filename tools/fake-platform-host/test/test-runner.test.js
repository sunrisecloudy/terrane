import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir, repoRoot } from "../src/paths.js";

test("checked-in example smoke tests run and persist test_runs", async () => {
  const host = new FakePlatformHost();
  try {
    const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
    for (const app of apps) {
      host.installPackage(path.join(examplesDir, app));
      const run = await host.runControlCommand("runtime.run_smoke_tests", { appId: app });
      assert.equal(run.status, "passed", `${app}: ${JSON.stringify(run.result.failures)}`);
      assert.equal(run.result.ok, true);
      assert.equal(run.result.total > 0, true);
    }

    const runs = await host.runControlCommand("db.query_test_runs", {});
    assert.equal(runs.length, apps.length);
    assert.equal(runs.every((run) => run.status === "passed"), true);
  } finally {
    host.close();
  }
});

test("checked-in microtests execute setup, validate statically, and persist runs", async () => {
  const microtestsDir = path.join(repoRoot, "tests", "micro");
  const files = fs.readdirSync(microtestsDir).filter((fileName) => fileName.endsWith(".microtest.json")).sort();

  for (const fileName of files) {
    const host = new FakePlatformHost();
    try {
      const run = await host.runControlCommand("runtime.run_microtest", {
        microtestPath: path.join("tests", "micro", fileName),
      });
      assert.equal(run.status, "passed", `${fileName}: ${JSON.stringify(run.result.failures)}`);
      assert.equal(run.result.setup.ok, true);
      assert.equal(run.result.teardown.ok, true);

      const appId = JSON.parse(fs.readFileSync(path.join(microtestsDir, fileName), "utf8")).targetApps[0];
      const persisted = await host.runControlCommand("db.query_test_runs", { appId });
      assert.equal(persisted.length, 1);
      assert.equal(persisted[0].status, "passed");
    } finally {
      host.close();
    }
  }
});

test("microtest failures are reported with stable repair codes", async () => {
  const host = new FakePlatformHost();
  try {
    const run = await host.runControlCommand("runtime.run_microtest", {
      spec: {
        id: "notes-lite-missing-selector",
        targetApps: ["notes-lite"],
        setup: [
          { tool: "platform.install_webapp_package", args: { path: "webapps/examples/notes-lite" } },
          { tool: "platform.open_webapp", args: { appId: "notes-lite" } },
        ],
        steps: [
          { tool: "runtime.click", args: { testId: "not-a-real-button" } },
          { tool: "runtime.assert_bridge_call", args: { method: "storage.set" } },
        ],
      },
    });

    assert.equal(run.status, "failed");
    assert.deepEqual(run.result.failures.map((failure) => failure.code), ["selector.not_found"]);
    const persisted = await host.runControlCommand("db.query_test_runs", { appId: "notes-lite" });
    assert.equal(persisted.length, 1);
    assert.equal(persisted[0].status, "failed");
  } finally {
    host.close();
  }
});
