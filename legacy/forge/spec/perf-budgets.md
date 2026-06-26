# Performance Budgets

Source of record: prd-merged/09 section 4, prd-merged/01 CR-5 and CR section 8, plus the committed forge-domain Limits defaults.

| Metric | Target | Source | Measurement | Gate |
|---|---|---|---|---|
| TS -> JS transpile | <5 ms typical for small applet | PRD09 section 4, CR-14 | Criterion benchmark over 1-file, 10-file, 100-file inputs; warm process | hard M0a |
| Applet cold start | <150 ms desktop; <400 ms web | PRD09 section 4 | Compile/transpile/load/run first entrypoint with empty input | hard M0a/M0b |
| Host call overhead | p95 <50 us native; <200 us web | PRD09 section 4 | Loop deterministic ctx calls through runtime harness | hard M0a |
| Indexed query latency | p95 <10 ms desktop; <50 ms web at 100k records | PRD09 section 4 | Query synthesized 100k record collection with index | hard M0b |
| Sync catch-up | to be refined | PRD09 section 4 | Replay CRDT/oplog delta from fixture frontier | soft until sync milestone |
| App cold start | to be refined per shell | PRD09 section 4 | Open workspace shell and mount first applet | soft until shell milestone |
| Native core size | <12 MB native binary, type-checker excluded | CR section 8 / PRD09 section 4 | Release build artifact size | hard release gate |
| Wasm core size | <=6 MB wasm gzipped, type-checker excluded | CR section 8 / PRD09 section 4 | wasm32 release build gzip size | hard release gate |
| Per-run wall limit | default 3000 ms | forge-domain Manifest::Limits | Runtime budget violation test | hard |
| Per-run fuel limit | default 10000000 | forge-domain Manifest::Limits | QuickJS fuel/op counter | hard |
| Per-run memory limit | default 67108864 bytes | forge-domain Manifest::Limits | Runtime allocator/memory cap | hard |
| Per-run host calls | default 10000 | forge-domain Manifest::Limits | Count ctx calls in a run | hard |
| Per-applet storage | default 10485760 bytes | forge-domain Manifest::Limits | KV/storage byte accounting | hard |
| Run log bytes | default 262144 bytes | forge-domain Manifest::Limits | Log capture truncation test | hard |

## Limits Defaults From forge-domain

- wall_ms: 3000
- fuel: 10000000
- memory_bytes: 67108864
- max_host_calls: 10000
- storage_bytes: 10485760
- log_bytes: 262144

## Budgets Still To Refine

Sync catch-up and whole app cold start are named gates in PRD09 but do not yet have concrete numbers. Keep them soft until the sync and shell harnesses exist, then promote them to hard gates with fixture-backed thresholds.
