import assert from "node:assert/strict";
import test from "node:test";
import { runFakeHostLatencyBenchmark } from "../../../tests/performance/fake-host-latency.mjs";

test("fake-host performance benchmark reports p50 and p95 latency", async () => {
  const report = await runFakeHostLatencyBenchmark({ warmup: 1, samples: 5, outputDir: null });

  assert.equal(report.ok, true);
  assert.equal(report.runner, "fake-host");
  assert.equal(report.methodology.reporting.includes("p50"), true);
  assert.equal(report.methodology.reporting.includes("p95"), true);
  assert.deepEqual(
    report.metrics.map((metric) => metric.id),
    ["storage_get_cached", "storage_set_1kib", "core_step_trivial"],
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
});
