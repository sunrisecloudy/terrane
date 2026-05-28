# Performance Harness

This directory contains benchmark harnesses for the host-level latency targets in `docs/22_RESOURCE_BUDGETS.md`.

`fake-host-latency.mjs` measures the reference fake host through the same dev control commands Codex uses:

- `runtime.storage_get`
- `runtime.storage_set`
- `runtime.core_step`

Default runs use the spec methodology: 50 warm-up iterations, 500 measured samples, and p50/p95 reporting. Use `--enforce-targets` in CI or release qualification when the host machine is quiet enough for timing assertions.
