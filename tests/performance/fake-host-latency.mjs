#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { FakePlatformHost } from "../../tools/fake-platform-host/src/fake-host.js";
import { examplesDir, repoRoot } from "../../tools/fake-platform-host/src/paths.js";

const DEFAULT_WARMUP = 50;
const DEFAULT_SAMPLES = 500;
const ONE_KIB = "x".repeat(1024);
const DESKTOP_TARGETS_MS = {
  storage_get_cached: { p50: 5, p95: 20 },
  storage_set_1kib: { p50: 10, p95: 40 },
  core_step_trivial: { p50: 5, p95: 20 },
};

export async function runFakeHostLatencyBenchmark({
  warmup = DEFAULT_WARMUP,
  samples = DEFAULT_SAMPLES,
  outputDir = null,
  enforceTargets = false,
} = {}) {
  const host = new FakePlatformHost();
  const packageDirs = [
    prepareBenchmarkPackage({ appId: "notes-lite", warmup, samples }),
    prepareBenchmarkPackage({ appId: "task-workbench", warmup, samples }),
  ];
  const startedAt = new Date().toISOString();
  try {
    for (const packageDir of packageDirs) {
      host.installPackage(packageDir);
    }

    const metrics = [
      await measureMetric({
        id: "storage_get_cached",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.storage_get_cached,
        run: (index) =>
          host.runControlCommand("runtime.storage_get", {
            appId: "notes-lite",
            key: "notes-lite:perf",
            defaultValue: { index },
          }),
      }),
      await measureMetric({
        id: "storage_set_1kib",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.storage_set_1kib,
        run: (index) =>
          host.runControlCommand("runtime.storage_set", {
            appId: "notes-lite",
            key: "notes-lite:perf",
            value: { index, payload: ONE_KIB },
          }),
      }),
      await measureMetric({
        id: "core_step_trivial",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.core_step_trivial,
        run: (index) =>
          host.runControlCommand("runtime.core_step", {
            appId: "task-workbench",
            event: { type: "BenchmarkTick", payload: { index } },
          }),
      }),
    ];

    const report = {
      ok: metrics.every((metric) => metric.samples === samples),
      targetStatus: metrics.every((metric) => metric.withinTarget) ? "pass" : "fail",
      varianceStatus: metrics.every((metric) => metric.varianceOk) ? "pass" : "needs-rerun",
      runner: "fake-host",
      methodology: {
        warmup,
        samples,
        reporting: ["p50", "p95"],
        targetProfile: "desktop",
      },
      startedAt,
      finishedAt: new Date().toISOString(),
      metrics,
    };

    if (outputDir) {
      fs.mkdirSync(outputDir, { recursive: true });
      fs.writeFileSync(path.join(outputDir, "fake-host-latency.json"), `${JSON.stringify(report, null, 2)}\n`);
    }

    if (enforceTargets && (report.targetStatus !== "pass" || report.varianceStatus !== "pass")) {
      report.ok = false;
    }

    return report;
  } finally {
    host.close();
    for (const packageDir of packageDirs) {
      fs.rmSync(packageDir, { recursive: true, force: true });
    }
  }
}

async function measureMetric({ id, warmup, samples, target, run }) {
  for (let index = 0; index < warmup; index += 1) {
    await run(index);
  }

  const durationsMs = [];
  for (let index = 0; index < samples; index += 1) {
    const start = performance.now();
    const result = await run(index);
    durationsMs.push(performance.now() - start);
    assertControlResult(id, result);
  }

  const stats = summarizeDurations(durationsMs);
  return {
    id,
    unit: "ms",
    samples,
    warmup,
    target,
    p50: round(stats.p50),
    p95: round(stats.p95),
    mean: round(stats.mean),
    stddev: round(stats.stddev),
    varianceRatio: round(stats.varianceRatio),
    varianceOk: stats.varianceRatio < 0.3,
    withinTarget: stats.p50 <= target.p50 && stats.p95 <= target.p95,
  };
}

function prepareBenchmarkPackage({ appId, warmup, samples }) {
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-perf-package-"));
  fs.cpSync(path.join(examplesDir, appId), packageDir, { recursive: true });
  const manifestPath = path.join(packageDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.resourceBudget = {
    ...manifest.resourceBudget,
    maxBridgeCallsPerMinute: Math.max(manifest.resourceBudget.maxBridgeCallsPerMinute, (warmup + samples) * 3 + 100),
  };
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return packageDir;
}

function assertControlResult(id, result) {
  if (result?.ok === false) {
    const code = result.error?.code ?? "unknown_error";
    throw new Error(`${id} control command failed: ${code}`);
  }
}

function summarizeDurations(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  const variance = values.reduce((sum, value) => sum + (value - mean) ** 2, 0) / values.length;
  const stddev = Math.sqrt(variance);
  return {
    p50: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
    mean,
    stddev,
    varianceRatio: mean === 0 ? 0 : stddev / mean,
  };
}

function percentile(sorted, q) {
  if (sorted.length === 0) return 0;
  const index = Math.ceil(sorted.length * q) - 1;
  return sorted[Math.max(0, Math.min(sorted.length - 1, index))];
}

function round(value) {
  return Math.round(value * 1000) / 1000;
}

function parseCliArgs(argv) {
  const options = {
    warmup: DEFAULT_WARMUP,
    samples: DEFAULT_SAMPLES,
    outputDir: path.join(repoRoot, "performance_runs"),
    enforceTargets: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--warmup") {
      options.warmup = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--samples") {
      options.samples = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--out") {
      options.outputDir = path.resolve(argv[(index += 1)]);
    } else if (arg === "--no-out") {
      options.outputDir = null;
    } else if (arg === "--enforce-targets") {
      options.enforceTargets = true;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (!Number.isInteger(options.warmup) || options.warmup < 0) {
    throw new Error("--warmup must be a non-negative integer");
  }
  if (!Number.isInteger(options.samples) || options.samples < 1) {
    throw new Error("--samples must be a positive integer");
  }
  return options;
}

const currentFile = fileURLToPath(import.meta.url);
if (process.argv[1] && path.resolve(process.argv[1]) === currentFile) {
  try {
    const report = await runFakeHostLatencyBenchmark(parseCliArgs(process.argv.slice(2)));
    console.log(JSON.stringify(report, null, 2));
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  }
}
