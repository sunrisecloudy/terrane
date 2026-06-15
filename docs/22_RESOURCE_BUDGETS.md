# Resource Budgets

## 1. Purpose

Generated apps can accidentally create infinite loops, huge DOM trees, excessive bridge calls, or runaway logs. Resource budgets make those failures observable and enforceable.

Schema: `schemas/resource-budget.schema.json`.

## 2. Manifest field

Every manifest must include:

```json
{
  "resourceBudget": {
    "maxDomNodes": 2000,
    "maxStorageBytes": 5242880,
    "maxBridgeCallsPerMinute": 600,
    "maxNetworkRequestsPerMinute": 60,
    "maxTimers": 64,
    "maxLogLinesPerMinute": 120,
    "maxPackageBytes": 1048576,
    "maxFileBytes": 524288
  }
}
```

The runtime clamps app-requested budgets down to platform defaults (§7).

## 3. Enforcement points

| Budget | Enforced by | When |
|---|---|---|
| package size | validator | install-time |
| file size | validator | install-time |
| DOM nodes | runtime sandbox manager | every 250 ms tick + on mutation |
| storage bytes | storage bridge | per `storage.set` call |
| bridge calls/minute | bridge dispatcher | sliding window |
| network calls/minute | network bridge | sliding window |
| timers | sandbox manager | on `setTimeout`/`setInterval` registration |
| log lines/minute | log bridge | sliding window |

Sliding-window counters are 60-second windows discretized into 6 ten-second buckets so old buckets drop off cleanly.

## 4. Violations

On violation, runtime returns:

```json
{
  "ok": false,
  "error": {
    "code": "RESOURCE_BUDGET_EXCEEDED",
    "message": "maxBridgeCallsPerMinute exceeded",
    "details": { "budget": "maxBridgeCallsPerMinute", "current": 601, "max": 600 }
  }
}
```

Repeated or severe violations trigger quarantine: three `RESOURCE_BUDGET_EXCEEDED` errors within 60 s quarantine the installed app version. The runtime emits `app.budget_warning` (docs/03 §1.1) when an app crosses 80% of any budget so AI repair can act before the hard cap.

## 5. Codex behavior

Codex must inspect resource usage after micro-tests:

```text
runtime.resource_usage
```

If an app exceeds a budget, Codex should patch the app to reduce work rather than request higher budgets unless the manifest and user approval are intentionally updated.

## 6. Platform clamps

The runtime clamps app-requested budgets down to per-platform maxima.

| Budget | Default (clamp ceiling) | Mobile clamp ceiling | Server clamp ceiling |
|---|---:|---:|---:|
| `maxDomNodes` | 5000 | 3000 | n/a |
| `maxStorageBytes` | 5 MiB | 5 MiB | 50 MiB |
| `maxBridgeCallsPerMinute` | 1200 | 600 | 6000 |
| `maxNetworkRequestsPerMinute` | 120 | 60 | 600 |
| `maxTimers` | 128 | 64 | n/a |
| `maxLogLinesPerMinute` | 300 | 120 | 1200 |
| `maxPackageBytes` | 4 MiB | 2 MiB | 16 MiB |
| `maxFileBytes` | 2 MiB | 1 MiB | 8 MiB |

A manifest may request a lower budget than the default. It may also request up to the clamp ceiling, but anything above the default flips `install_report.requiresUserApproval = true`.

## 7. Performance budgets (host-level latency)

These are not per-app budgets; they describe what the platform itself must meet so generated apps feel responsive.

### 7.1 Measurement methodology

- **Tool**: a benchmark harness lives under `tests/performance/` and uses the dev control plane (`runtime.bridge_calls`, `runtime.core_step`) to time round-trips.
- **Warm-up**: 50 untimed iterations precede every measurement run.
- **Sample size**: 500 iterations per metric per platform.
- **Reporting**: p50 and p95 reported; both must meet target.
- **Variance check**: standard deviation must be < 30% of the mean; otherwise the run is invalid and must be rerun on a quieter machine.
- **Run conditions**: no other applications active; CPU governor in performance mode (Linux); macOS in low-power-mode off; mobile devices plugged in.

### 7.2 Targets

| Metric | Desktop p50 | Desktop p95 | Mobile p50 | Mobile p95 |
|---|---:|---:|---:|---:|
| Runtime launcher initial load | 400 ms | 1000 ms | 800 ms | 2000 ms |
| Example app cold load (after runtime ready) | 200 ms | 500 ms | 350 ms | 1000 ms |
| Bridge round-trip `storage.get` (cached) | 5 ms | 20 ms | 10 ms | 50 ms |
| Bridge round-trip `storage.set` 1 KiB | 10 ms | 40 ms | 20 ms | 80 ms |
| `core.step` round-trip for a trivial event | 5 ms | 20 ms | 12 ms | 50 ms |
| 1000-row virtual list first paint | n/a | n/a | n/a | n/a (no jank — visual check) |
| Memory after opening all 5 examples | unbounded growth not allowed | | | |
| Bridge call/minute throughput | 1200 sustained | | 600 sustained | |

Misses in any p95 are CI-failing on the affected platform.

### 7.3 Per-platform context

- **iOS / macOS WKWebView**: bridge round-trip is dominated by JSON encode/decode and main-thread dispatch. Avoid synchronous bridges.
- **Android WebView**: `WebMessageListener` callbacks land on a background thread; serialize to main only when touching UI. JNI calls into Forge dominate `core.step` cost.
- **WebView2**: cold start can spike if WebView2 runtime is uninstalled — measurement assumes WebView2 is installed.
- **WebKitGTK**: GTK4 main loop interleaves UI and message dispatch; expect higher variance.
- **Server**: HTTP overhead replaces WebView IPC; p95 is much lower.

## 8. Reporting

Performance and budget reports are stored in:

- `app_install_reports.budgets_audit` for install-time audits.
- `runtime_sessions.resource_high_water` for runtime sessions.
- A separate `performance_runs/` directory in CI artifacts for benchmark results.
