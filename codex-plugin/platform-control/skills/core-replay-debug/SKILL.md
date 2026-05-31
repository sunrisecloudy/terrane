---
name: core-replay-debug
description: Debug deterministic Zig core behavior by replaying runtime/core event logs, comparing snapshots, and verifying fixes through core and bridge contract tests.
---

# Core Replay Debug

Use this skill when core state, `core.step`, action logs, replay output, or runtime/core snapshot comparisons disagree.

## Workflow

1. Capture the evidence with `runtime.event_log`, `runtime.core_snapshot`, `runtime.bridge_calls`, and `db.query_core_events`.
2. Reproduce with `runtime.replay_events` or `runtime.core_step`; prefer checked-in golden or replay fixtures when available.
3. Compare expected and actual actions with `runtime.assert_core_action` and `runtime.compare_snapshot`.
4. If the defect is in Zig core, keep the fix deterministic: event in, action/state out, no platform effects in core logic.
5. If the defect is in bridge wiring, update bridge fixtures and contract tests rather than only patching one host.
6. Verify with Zig unit/replay tests and the relevant reference-host or server bridge contract tests.

## Guardrails

- Do not hide nondeterminism with timing sleeps; replay must be deterministic.
- Do not add native or async effects to core logic.
- Use `platform.create_snapshot` before migration or rollback debugging that may change persisted app data.
- If a verified behavior changes status, update `IMPLEMENTATION_STATUS.md`; if an acceptance box becomes verified, update `docs/10_ACCEPTANCE_CHECKLIST.md`.
