import assert from "node:assert/strict";
import test from "node:test";
import { applyPerformanceEnforcement, runReferenceHostLatencyBenchmark } from "../../../tests/performance/reference-host-latency.mjs";

test("reference-host performance benchmark reports p50 and p95 latency", async () => {
  const report = await runReferenceHostLatencyBenchmark({
    warmup: 1,
    samples: 5,
    lifecycleLoops: 3,
    throughputCalls: 10,
    outputDir: null,
  });

  assert.equal(report.ok, true);
  assert.equal(report.runner, "reference-host");
  assert.equal(report.scenarioStatus, "pass");
  assert.equal(report.methodology.reporting.includes("p50"), true);
  assert.equal(report.methodology.reporting.includes("p95"), true);
  assert.deepEqual(
    report.metrics.map((metric) => metric.id),
    [
      "runtime_launcher_initial_load",
      "example_app_open_idle",
      "app_switch_open_idle",
      "storage_get_cached",
      "storage_set_1kib",
      "core_step_trivial",
    ],
  );
  assert.deepEqual(
    report.scenarios.map((scenario) => scenario.id),
    ["network_timeout", "bridge_throughput", "open_all_examples_memory", "large_list", "install_uninstall_loop"],
  );

  for (const metric of report.metrics) {
    assert.equal(metric.samples, 5);
    assert.equal(metric.warmup, 1);
    assert.equal(Number.isFinite(metric.p50), true);
    assert.equal(Number.isFinite(metric.p95), true);
    assert.equal(metric.p95 >= metric.p50, true);
    assert.equal("p50" in metric.target, true);
    assert.equal("p95" in metric.target, true);
  }

  const timeout = report.scenarios.find((scenario) => scenario.id === "network_timeout");
  assert.equal(timeout.ok, true);
  assert.equal(timeout.actualTimeoutMs, timeout.expectedTimeoutMs);

  const throughput = report.scenarios.find((scenario) => scenario.id === "bridge_throughput");
  assert.equal(throughput.ok, true);
  assert.equal(throughput.calls, 10);
  assert.equal(throughput.callsPerMinute >= throughput.targetCallsPerMinute, true);

  const memory = report.scenarios.find((scenario) => scenario.id === "open_all_examples_memory");
  assert.equal(memory.ok, true);
  assert.equal(memory.appCount, 5);
  assert.equal(memory.sessionDelta, 5);
  assert.deepEqual(memory.openedAppIds.sort(), [
    "api-dashboard",
    "core-replay-lab",
    "file-transformer",
    "notes-lite",
    "task-workbench",
  ]);

  const largeList = report.scenarios.find((scenario) => scenario.id === "large_list");
  assert.equal(largeList.ok, true);
  assert.equal(largeList.rowCount, 1000);
  assert.equal(largeList.pageSize, 40);
  assert.equal(largeList.renderedRows, 40);
  assert.equal(largeList.hasWindowedSlice, true);
  assert.equal(largeList.storageBytes < largeList.maxStorageBytes, true);

  const lifecycle = report.scenarios.find((scenario) => scenario.id === "install_uninstall_loop");
  assert.equal(lifecycle.ok, true);
  assert.equal(lifecycle.loops, 3);
  assert.deepEqual(lifecycle.logicalResidueFailures, []);
});

test("performance target enforcement keeps variance-only rerun signals separate", () => {
  const varianceOnly = applyPerformanceEnforcement(
    { ok: true, targetStatus: "pass", varianceStatus: "needs-rerun", scenarioStatus: "pass" },
    { enforceTargets: true },
  );
  assert.equal(varianceOnly.ok, true);

  const targetMiss = applyPerformanceEnforcement(
    { ok: true, targetStatus: "fail", varianceStatus: "pass", scenarioStatus: "pass" },
    { enforceTargets: true },
  );
  assert.equal(targetMiss.ok, false);

  const releaseVarianceGate = applyPerformanceEnforcement(
    { ok: true, targetStatus: "pass", varianceStatus: "needs-rerun", scenarioStatus: "pass" },
    { enforceVariance: true },
  );
  assert.equal(releaseVarianceGate.ok, false);
});
