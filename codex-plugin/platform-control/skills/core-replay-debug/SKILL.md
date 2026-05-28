---
name: core-replay-debug
description: Use this when debugging Zig core event/action determinism, replay failures, or mismatches between generated app behavior and core state.
---

# Core replay debug skill

Use runtime/core logs and replay tools to debug deterministic Zig core behavior.

## Workflow

1. Read the runtime event log and bridge calls.
2. Extract all `core.step` events in order.
3. Call `runtime.replay_events` or run Zig replay tests.
4. Compare expected actions, final snapshot hash, and observed host behavior.
5. If mismatch is in generated app code, patch the app.
6. If mismatch is in core behavior, patch Zig tests first, then Zig code.
7. Re-run replay and app micro-tests.

## Rules

- Do not inspect or mutate private core state except through dev-only snapshot APIs.
- Treat event logs as the source of truth for reproduction.
- Add regression fixtures for every replay bug.
