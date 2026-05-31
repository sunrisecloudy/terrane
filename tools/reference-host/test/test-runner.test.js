import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { BrowserSmokeRunner } from "../src/browser-smoke-runner.js";
import { PlatformError } from "../src/errors.js";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir, repoRoot } from "../src/paths.js";

test("checked-in example smoke tests run and persist test_runs", async () => {
  const host = new ReferenceHost();
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
    assert.equal(runs.length, apps.length * 2);
    assert.equal(runs.every((run) => run.status === "passed"), true);
  } finally {
    host.close();
  }
});

test("runtime.run_smoke_tests can delegate to browser-backed runner", async () => {
  let called = false;
  const host = new ReferenceHost({
    browserSmokeRunner: {
      async run({ appId, tests, files }) {
        called = true;
        assert.equal(appId, "notes-lite");
        assert.equal(files.has("index.html"), true);
        return {
          ok: true,
          appId,
          total: tests.length,
          assertions: 7,
          failures: [],
          runner: "browser",
          browser: { engine: "stub" },
          bridgeCalls: [{ method: "storage.set", id: "stub_req" }],
        };
      },
    },
  });
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    const run = await host.runControlCommand("runtime.run_smoke_tests", { appId: "notes-lite", runner: "browser" });
    assert.equal(called, true);
    assert.equal(run.status, "passed");
    assert.equal(run.result.runner, "browser");

    const persisted = await host.runControlCommand("db.query_test_runs", { appId: "notes-lite" });
    assert.equal(persisted.some((row) => row.micro_test_id === "smoke:notes-lite" && row.status === "passed"), true);
  } finally {
    host.close();
  }
});

test("runtime.run_smoke_tests auto mode falls back when browser is unavailable", async () => {
  const host = new ReferenceHost({
    browserSmokeRunner: {
      async run() {
        throw new PlatformError("browser_smoke_unavailable", "No test browser", {});
      },
    },
  });
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    const run = await host.runControlCommand("runtime.run_smoke_tests", { appId: "notes-lite", runner: "auto" });
    assert.equal(run.status, "passed");
    assert.equal(run.result.runner, "static");
    assert.equal(run.result.fallback.code, "browser_smoke_unavailable");
  } finally {
    host.close();
  }
});

test(
  "runtime.run_smoke_tests browser mode executes every example in a real browser",
  { skip: process.env.NATIVE_AI_ENABLE_BROWSER_SMOKE_TESTS !== "1" || !BrowserSmokeRunner.isAvailable() },
  async () => {
    const host = new ReferenceHost();
    try {
      const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
      for (const app of apps) {
        host.installPackage(path.join(examplesDir, app));
        const run = await host.runControlCommand("runtime.run_smoke_tests", { appId: app, runner: "browser" });
        assert.equal(run.status, "passed", `${app}: ${JSON.stringify(run.result.failures)}`);
        assert.equal(run.result.runner, "browser");
        assert.equal(run.result.bridgeCalls.length > 0, true, `${app}: expected bridge calls`);
      }
    } finally {
      host.close();
    }
  },
);

test("checked-in microtests execute setup, validate statically, and persist runs", async () => {
  const microtestsDir = path.join(repoRoot, "tests", "micro");
  const files = fs.readdirSync(microtestsDir).filter((fileName) => fileName.endsWith(".microtest.json")).sort();

  for (const fileName of files) {
    const host = new ReferenceHost();
    try {
      const run = await host.runControlCommand("runtime.run_microtest", {
        microtestPath: path.join("tests", "micro", fileName),
      });
      assert.equal(run.status, "passed", `${fileName}: ${JSON.stringify(run.result.failures)}`);
      assert.equal(run.result.setup.ok, true);
      assert.equal(run.result.teardown.ok, true);

      const spec = JSON.parse(fs.readFileSync(path.join(microtestsDir, fileName), "utf8"));
      const appId = spec.targetApps[0];
      const persisted = await host.runControlCommand("db.query_test_runs", { appId });
      assert.equal(persisted.some((run) => run.micro_test_id === spec.id), true);
      assert.equal(persisted.every((run) => run.status === "passed"), true);
    } finally {
      host.close();
    }
  }
});

test("checked-in golden flows execute as reference-host microtests", async () => {
  const goldenDir = path.join(repoRoot, "tests", "golden");
  const files = fs.readdirSync(goldenDir).filter((fileName) => fileName.endsWith(".golden.json")).sort();
  assert.deepEqual(files, [
    "core-step.golden.json",
    "file-dialog-core.golden.json",
    "form-storage.golden.json",
    "large-table.golden.json",
    "network-policy.golden.json",
  ]);

  for (const fileName of files) {
    const host = new ReferenceHost();
    try {
      const run = await host.runControlCommand("runtime.run_microtest", {
        microtestPath: path.join("tests", "golden", fileName),
      });
      assert.equal(run.status, "passed", `${fileName}: ${JSON.stringify(run.result.failures)}`);
      assert.equal(run.result.setup.ok, true, `${fileName}: ${JSON.stringify(run.result.setup.failures)}`);

      const spec = JSON.parse(fs.readFileSync(path.join(goldenDir, fileName), "utf8"));
      const appId = spec.targetApps[0];
      const persisted = await host.runControlCommand("db.query_test_runs", { appId });
      assert.equal(persisted.some((row) => row.micro_test_id === spec.id && row.status === "passed"), true, fileName);
    } finally {
      host.close();
    }
  }
});

test("microtest failures are reported with stable repair codes", async () => {
  const host = new ReferenceHost();
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
    assert.equal(persisted.some((row) => row.status === "failed" && row.micro_test_id === "notes-lite-missing-selector"), true);
  } finally {
    host.close();
  }
});
