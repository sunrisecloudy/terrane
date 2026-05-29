import assert from "node:assert/strict";
import test from "node:test";
import { runFakeHostLatencyBenchmark } from "../../../tests/performance/fake-host-latency.mjs";

test("fake-host performance benchmark reports p50 and p95 latency", async () => {
  const report = await runFakeHostLatencyBenchmark({ warmup: 1, samples: 5, lifecycleLoops: 3, outputDir: null });

  assert.equal(report.ok, true);
  assert.equal(report.runner, "fake-host");
  assert.equal(report.scenarioStatus, "pass");
  assert.equal(report.methodology.reporting.includes("p50"), true);
  assert.equal(report.methodology.reporting.includes("p95"), true);
  assert.deepEqual(
    report.metrics.map((metric) => metric.id),
    ["example_app_open_idle", "app_switch_open_idle", "storage_get_cached", "storage_set_1kib", "core_step_trivial"],
  );
  assert.deepEqual(
    report.scenarios.map((scenario) => scenario.id),
    ["network_timeout", "install_uninstall_loop"],
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

  const lifecycle = report.scenarios.find((scenario) => scenario.id === "install_uninstall_loop");
  assert.equal(lifecycle.ok, true);
  assert.equal(lifecycle.loops, 3);
  assert.deepEqual(lifecycle.logicalResidueFailures, []);
});
