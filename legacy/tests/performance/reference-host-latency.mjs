#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { ReferenceHost } from "../../tools/reference-host/src/reference-host.js";
import { examplesDir, repoRoot } from "../../tools/reference-host/src/paths.js";

const DEFAULT_WARMUP = 50;
const DEFAULT_SAMPLES = 500;
const DEFAULT_LIFECYCLE_LOOPS = 50;
const DEFAULT_THROUGHPUT_CALLS = 1200;
const DESKTOP_BRIDGE_THROUGHPUT_TARGET_PER_MINUTE = 1200;
const ONE_KIB = "x".repeat(1024);
const MEMORY_GROWTH_LIMIT_BYTES = 50 * 1024 * 1024;
const NETWORK_TIMEOUT_TOLERANCE = 0.1;
const DESKTOP_TARGETS_MS = {
  runtime_launcher_initial_load: { p50: 400, p95: 1000 },
  example_app_open_idle: { p50: 200, p95: 500 },
  app_switch_open_idle: { p50: 200, p95: 500 },
  storage_get_cached: { p50: 5, p95: 20 },
  storage_set_1kib: { p50: 10, p95: 40 },
  core_step_trivial: { p50: 5, p95: 20 },
};
const EXAMPLE_APP_IDS = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab", "calendar-planner"];

export async function runReferenceHostLatencyBenchmark({
  warmup = DEFAULT_WARMUP,
  samples = DEFAULT_SAMPLES,
  lifecycleLoops = DEFAULT_LIFECYCLE_LOOPS,
  throughputCalls = DEFAULT_THROUGHPUT_CALLS,
  outputDir = null,
  enforceTargets = false,
  enforceVariance = false,
} = {}) {
  const host = new ReferenceHost();
  const packageDirs = EXAMPLE_APP_IDS.map((appId) =>
    prepareBenchmarkPackage({ appId, warmup, samples, lifecycleLoops, throughputCalls }),
  );
  const startedAt = new Date().toISOString();
  try {
    for (const packageDir of packageDirs) {
      host.installPackage(packageDir);
    }

    const metrics = [
      await measureMetric({
        id: "runtime_launcher_initial_load",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.runtime_launcher_initial_load,
        run: () => listLauncherApps(host),
      }),
      await measureMetric({
        id: "example_app_open_idle",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.example_app_open_idle,
        run: () => openAndWait(host, "notes-lite"),
      }),
      await measureMetric({
        id: "app_switch_open_idle",
        warmup,
        samples,
        target: DESKTOP_TARGETS_MS.app_switch_open_idle,
        run: async (index) => {
          await openAndWait(host, index % 2 === 0 ? "notes-lite" : "task-workbench");
          return openAndWait(host, index % 2 === 0 ? "task-workbench" : "notes-lite");
        },
      }),
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
    const scenarios = [
      await runNetworkTimeoutScenario(host),
      await runBridgeThroughputScenario({ host, appId: "notes-lite", calls: throughputCalls }),
      await runOpenAllExamplesMemoryScenario({ host, appIds: EXAMPLE_APP_IDS }),
      await runLargeListScenario({ host, appId: "task-workbench" }),
      await runInstallUninstallScenario({
        host,
        packageDir: packageDirs[0],
        appId: "notes-lite",
        loops: lifecycleLoops,
      }),
    ];

    const report = {
      ok: metrics.every((metric) => metric.samples === samples) && scenarios.every((scenario) => scenario.ok),
      targetStatus: metrics.every((metric) => metric.withinTarget) ? "pass" : "fail",
      varianceStatus: metrics.every((metric) => metric.varianceOk) ? "pass" : "needs-rerun",
      scenarioStatus: scenarios.every((scenario) => scenario.ok) ? "pass" : "fail",
      runner: "reference-host",
      methodology: {
        warmup,
        samples,
        lifecycleLoops,
        throughputCalls,
        reporting: ["p50", "p95"],
        targetProfile: "desktop",
      },
      startedAt,
      finishedAt: new Date().toISOString(),
      metrics,
      scenarios,
    };

    if (outputDir) {
      fs.mkdirSync(outputDir, { recursive: true });
      fs.writeFileSync(path.join(outputDir, "reference-host-latency.json"), `${JSON.stringify(report, null, 2)}\n`);
    }

    return applyPerformanceEnforcement(report, { enforceTargets, enforceVariance });
  } finally {
    host.close();
    for (const packageDir of packageDirs) {
      fs.rmSync(packageDir, { recursive: true, force: true });
    }
  }
}

export function applyPerformanceEnforcement(report, { enforceTargets = false, enforceVariance = false } = {}) {
  if (enforceTargets && report.targetStatus !== "pass") {
    report.ok = false;
  }
  if (enforceVariance && report.varianceStatus !== "pass") {
    report.ok = false;
  }
  return report;
}

async function listLauncherApps(host) {
  const result = await host.runControlCommand("platform.list_webapps", {});
  assertControlResult("runtime_launcher_initial_load", result);
  if (!Array.isArray(result.apps) || result.apps.length < EXAMPLE_APP_IDS.length) {
    throw new Error("runtime launcher app list did not include bundled examples");
  }
  return result;
}

async function openAndWait(host, appId) {
  const opened = await host.runControlCommand("platform.open_webapp", { appId });
  assertControlResult(`open_${appId}`, opened);
  const idle = await host.runControlCommand("runtime.wait_for", {
    appId,
    sessionId: opened.sessionId,
    kind: "idle",
  });
  assertControlResult(`wait_for_${appId}`, idle);
  return { ok: true, appId, sessionId: opened.sessionId };
}

async function runNetworkTimeoutScenario(host) {
  const expectedTimeoutMs = 10;
  const delayMs = 50;
  await host.runControlCommand("runtime.network_mock_set", {
    appId: "api-dashboard",
    method: "GET",
    urlPattern: "https://api.example.com/slow",
    response: { status: 200, headers: {}, bodyText: "ok", delayMs },
  });
  const response = await host.runControlCommand("runtime.call_bridge", {
    appId: "api-dashboard",
    method: "network.request",
    params: {
      url: "https://api.example.com/slow",
      method: "GET",
      headers: {},
      body: null,
      timeoutMs: expectedTimeoutMs,
    },
  });
  const actualTimeoutMs = response?.error?.details?.timeoutMs;
  const driftRatio = Math.abs(actualTimeoutMs - expectedTimeoutMs) / expectedTimeoutMs;
  return {
    id: "network_timeout",
    ok: response?.ok === false && response.error?.code === "timeout" && driftRatio <= NETWORK_TIMEOUT_TOLERANCE,
    expectedTimeoutMs,
    actualTimeoutMs,
    delayMs: response?.error?.details?.delayMs ?? null,
    toleranceRatio: NETWORK_TIMEOUT_TOLERANCE,
    driftRatio: round(driftRatio),
  };
}

async function runBridgeThroughputScenario({ host, appId, calls }) {
  const opened = await openAndWait(host, appId);
  const started = performance.now();
  for (let index = 0; index < calls; index += 1) {
    const result = await host.runControlCommand("runtime.storage_get", {
      appId,
      sessionId: opened.sessionId,
      key: `${appId}:throughput`,
      defaultValue: null,
    });
    assertControlResult("bridge_throughput", result);
  }
  const elapsedMs = performance.now() - started;
  const callsPerMinute = elapsedMs === 0 ? Number.POSITIVE_INFINITY : (calls / elapsedMs) * 60_000;
  return {
    id: "bridge_throughput",
    ok: callsPerMinute >= DESKTOP_BRIDGE_THROUGHPUT_TARGET_PER_MINUTE,
    calls,
    elapsedMs: round(elapsedMs),
    callsPerMinute: Math.round(callsPerMinute),
    targetCallsPerMinute: DESKTOP_BRIDGE_THROUGHPUT_TARGET_PER_MINUTE,
  };
}

async function runOpenAllExamplesMemoryScenario({ host, appIds }) {
  const beforeHeapBytes = process.memoryUsage().heapUsed;
  const beforeSessions = runtimeSessionCount(host.database);
  const opened = [];

  for (const appId of appIds) {
    opened.push(await openAndWait(host, appId));
  }

  const heapDeltaBytes = process.memoryUsage().heapUsed - beforeHeapBytes;
  const sessionDelta = runtimeSessionCount(host.database) - beforeSessions;
  const openedAppIds = [...new Set(opened.map((entry) => entry.appId))];
  const boundedMemoryGrowth = heapDeltaBytes <= MEMORY_GROWTH_LIMIT_BYTES;

  return {
    id: "open_all_examples_memory",
    ok: boundedMemoryGrowth && openedAppIds.length === appIds.length && sessionDelta === appIds.length,
    appCount: appIds.length,
    openedAppIds,
    sessionDelta,
    boundedMemoryGrowth,
    heapDeltaBytes,
    memoryGrowthLimitBytes: MEMORY_GROWTH_LIMIT_BYTES,
  };
}

async function runLargeListScenario({ host, appId }) {
  const rowCount = 1000;
  const packageRecord = host.database.activeInstallPackage(appId);
  const appJs = packageRecord.files.get("app.js") ?? "";
  const pageSize = extractTaskPageSize(appJs);
  const hasWindowedSlice = /filtered\.slice\(start,start\+PAGE_SIZE\)/.test(appJs);
  const rows = Array.from({ length: rowCount }, (_, index) => ({
    id: `perf_task_${index}`,
    title: `Large list task ${index + 1}`,
    priority: index % 10 === 0 ? "high" : index % 3 === 0 ? "low" : "medium",
    done: index % 5 === 0,
    createdAt: index,
  }));
  const storage = await host.runControlCommand("runtime.storage_set", {
    appId,
    key: `${appId}:tasks`,
    value: rows,
  });
  assertControlResult("large_list_storage_set", storage);
  await openAndWait(host, appId);
  const usage = await host.runControlCommand("runtime.resource_usage", { appId });
  const maxStorageBytes = packageRecord.manifest.resourceBudget.maxStorageBytes;
  const renderedRows = Math.min(rowCount, pageSize ?? rowCount);
  return {
    id: "large_list",
    ok: Boolean(pageSize && pageSize <= 100 && hasWindowedSlice && usage.storageBytes > 0 && usage.storageBytes < maxStorageBytes),
    rowCount,
    pageSize,
    renderedRows,
    hasWindowedSlice,
    storageBytes: usage.storageBytes,
    maxStorageBytes,
  };
}

function extractTaskPageSize(source) {
  const match = source.match(/\bPAGE_SIZE\s*=\s*(\d+)/);
  return match ? Number.parseInt(match[1], 10) : null;
}

async function runInstallUninstallScenario({ host, packageDir, appId, loops }) {
  const beforeCounts = tableCounts(host.database);
  const beforeHeapBytes = process.memoryUsage().heapUsed;
  const residueFailures = [];

  for (let index = 0; index < loops; index += 1) {
    const install = host.installPackage(packageDir);
    if (install.status !== "enabled") {
      residueFailures.push({ index, code: "install_not_enabled", status: install.status });
      continue;
    }
    const opened = await openAndWait(host, appId);
    const storage = await host.runControlCommand("runtime.storage_set", {
      appId,
      sessionId: opened.sessionId,
      key: `${appId}:perf-loop`,
      value: { index },
    });
    if (storage?.ok === false) {
      residueFailures.push({ index, code: "storage_set_failed", error: storage.error });
    }
    const uninstall = await host.runControlCommand("platform.uninstall_webapp", {
      appId,
      confirm: true,
      actor: "performance-harness",
    });
    if (uninstall.status !== "uninstalled") {
      residueFailures.push({ index, code: "uninstall_failed", status: uninstall.status });
    }
    const active = activeAppState(host.database, appId);
    if (active.activeInstallId || active.enabledVersions !== 0 || active.storageRows !== 0) {
      residueFailures.push({ index, code: "logical_residue", active });
    }
  }

  const afterCounts = tableCounts(host.database);
  const heapDeltaBytes = process.memoryUsage().heapUsed - beforeHeapBytes;
  const boundedMemoryGrowth = heapDeltaBytes <= MEMORY_GROWTH_LIMIT_BYTES;
  return {
    id: "install_uninstall_loop",
    ok: residueFailures.length === 0 && boundedMemoryGrowth,
    loops,
    boundedMemoryGrowth,
    heapDeltaBytes,
    memoryGrowthLimitBytes: MEMORY_GROWTH_LIMIT_BYTES,
    logicalResidueFailures: residueFailures,
    tableDeltas: diffCounts(beforeCounts, afterCounts),
  };
}

function runtimeSessionCount(database) {
  return database.get("SELECT COUNT(*) AS count FROM runtime_sessions").count;
}

function tableCounts(database) {
  const tables = [
    "apps",
    "app_versions",
    "app_files",
    "app_permissions",
    "app_storage",
    "app_install_reports",
    "app_installations",
    "runtime_sessions",
    "runtime_snapshots",
    "test_runs",
  ];
  return Object.fromEntries(
    tables.map((table) => [table, database.get(`SELECT COUNT(*) AS count FROM ${table}`).count]),
  );
}

function activeAppState(database, appId) {
  const app = database.get("SELECT status, active_install_id FROM apps WHERE id = ?", appId);
  return {
    status: app?.status ?? null,
    activeInstallId: app?.active_install_id ?? null,
    enabledVersions: database.get("SELECT COUNT(*) AS count FROM app_versions WHERE app_id = ? AND status = 'enabled'", appId).count,
    storageRows: database.get("SELECT COUNT(*) AS count FROM app_storage WHERE app_id = ?", appId).count,
  };
}

function diffCounts(before, after) {
  return Object.fromEntries(Object.keys(after).map((key) => [key, after[key] - (before[key] ?? 0)]));
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

function prepareBenchmarkPackage({ appId, warmup, samples, lifecycleLoops, throughputCalls }) {
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-perf-package-"));
  fs.cpSync(path.join(examplesDir, appId), packageDir, { recursive: true });
  const manifestPath = path.join(packageDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const bridgeCallBudget = (warmup + samples) * 4 + lifecycleLoops + throughputCalls + 250;
  manifest.resourceBudget = {
    ...manifest.resourceBudget,
    maxBridgeCallsPerMinute: Math.max(manifest.resourceBudget.maxBridgeCallsPerMinute, bridgeCallBudget),
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
    lifecycleLoops: DEFAULT_LIFECYCLE_LOOPS,
    throughputCalls: DEFAULT_THROUGHPUT_CALLS,
    outputDir: path.join(repoRoot, "performance_runs"),
    enforceTargets: false,
    enforceVariance: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--warmup") {
      options.warmup = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--samples") {
      options.samples = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--lifecycle-loops") {
      options.lifecycleLoops = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--throughput-calls") {
      options.throughputCalls = Number.parseInt(argv[(index += 1)], 10);
    } else if (arg === "--out") {
      options.outputDir = path.resolve(argv[(index += 1)]);
    } else if (arg === "--no-out") {
      options.outputDir = null;
    } else if (arg === "--enforce-targets") {
      options.enforceTargets = true;
    } else if (arg === "--enforce-variance") {
      options.enforceVariance = true;
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
  if (!Number.isInteger(options.lifecycleLoops) || options.lifecycleLoops < 1) {
    throw new Error("--lifecycle-loops must be a positive integer");
  }
  if (!Number.isInteger(options.throughputCalls) || options.throughputCalls < 1) {
    throw new Error("--throughput-calls must be a positive integer");
  }
  return options;
}

const currentFile = fileURLToPath(import.meta.url);
if (process.argv[1] && path.resolve(process.argv[1]) === currentFile) {
  try {
    const report = await runReferenceHostLatencyBenchmark(parseCliArgs(process.argv.slice(2)));
    console.log(JSON.stringify(report, null, 2));
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  }
}
