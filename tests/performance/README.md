# Performance Harness

This directory contains benchmark harnesses for the host-level latency targets in `docs/22_RESOURCE_BUDGETS.md`.

`fake-host-latency.mjs` measures the reference fake host through the same dev control commands Codex uses:

- `platform.open_webapp` + `runtime.wait_for` app-open/app-switch timing
- `runtime.storage_get`
- `runtime.storage_set`
- `runtime.core_step`
- bridge call/minute throughput over `runtime.storage_get`
- bounded memory growth after opening all five bundled examples
- `task-workbench` large-list windowing with 1000 stored rows
- `runtime.call_bridge` for `network.request` timeout enforcement
- `platform.uninstall_webapp` install/uninstall loop cleanup

Default runs use the spec methodology: 50 warm-up iterations, 500 measured samples, 1200 throughput calls, 50 install/uninstall lifecycle loops, and p50/p95 reporting. Use `--enforce-targets` in CI or release qualification when the host machine is quiet enough for timing assertions.
