---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/e2e/*/  (each: applet.ts, manifest.json, input.json, expect.json)
---

# T018 — End-to-end spine scenarios (for forge-core + forge-cli)

The next workflow builds `forge-core` (the command/event facade) and `forge-cli`
(`forge demo` + the M0a acceptance test). Beyond the single `notes-lite` demo, I
want a set of end-to-end scenarios that exercise the WHOLE spine
(install → run → SQLite write → UI tree → deterministic replay) so the core's
integration tests have real coverage.

## Deliverable

`forge/fixtures/e2e/<scenario>/` directories, each containing:
- `applet.ts` — a small TS applet authored against `@forge/std`
  (`forge/std/forge-std.d.ts`): uses `ctx.db` / `ctx.storage` / `ctx.ui` /
  `ctx.time` / `ctx.random`, exports `async function main(ctx, input)`.
- `manifest.json` — capabilities + limits (see `forge/crates/domain/src/manifest.rs`).
- `input.json` — the `input` passed to `main`.
- `expect.json` — the expected observable outcome:
  ```json
  { "result": { "ok": true, "value": {...} },
    "records": [ { "collection": "notes", "fields": {...} } ],
    "ui_contains": ["Text:My Notes", "List"],
    "replay_identical": true }
  ```
  (`ui_contains` = substrings/markers the emitted tree must include — keep it
  loose; the exact patch shape is the ui crate's concern.)

## Scenarios (~8)

1. `note_taker` — insert a record from input, list, render a Stack+List (the canonical demo).
2. `counter` — read `ctx.storage` counter, increment, write back, render a Stat/Text.
3. `seeded_random` — use `ctx.random` to pick from a list deterministically; same seed → same pick (replay-identical).
4. `multi_insert` — insert several records in one run; assert all are stored + listed.
5. `form_echo` — take input fields, render a Form-ish Stack of Text echoing them.
6. `denied_capability` — applet tries `ctx.db.insert` into a collection its manifest does NOT grant → run outcome is PermissionDenied, NO record written. (`expect.json` has `"error_code": "PermissionDenied"`.)
7. `rejected_eval` — applet source contains `eval(...)` → install/compile REJECTED (pipeline scan), applet never runs. (`expect.json` has `"install_rejected": true`.)
8. `time_log` — read `ctx.time.now()` twice, assert monotone logical clock; render the values.

## Notes

Keep applets deterministic (no live network/time). In `## Result`, note any scenario
whose expected outcome you couldn't pin down from the current crate APIs so I refine
the `expect.json` schema before wiring these as CI gates. These complement, not
replace, the `notes-lite` demo the cli workflow will create.

## Result

Delivered eight scenario directories under `forge/fixtures/e2e/`, each with
`applet.ts`, `manifest.json`, `input.json`, and `expect.json`:
`note_taker`, `counter`, `seeded_random`, `multi_insert`, `form_echo`,
`denied_capability`, `rejected_eval`, and `time_log`.

Notes for harness wiring:
- `counter` renders `Text` rather than `Stat` because the current
  `forge/std/forge-std.d.ts` public target only exposes `Stack`, `Text`,
  `Button`, `TextField`, and `List`.
- `expect.json` stays intentionally loose (`ui_contains`, records, replay flags)
  so forge-core/forge-cli can decide the exact acceptance shape.
- `denied_capability` expects `PermissionDenied` and no committed records.
- `rejected_eval` is an install/compile-stage rejection; the applet should never
  run.

Verification:
- Parsed all e2e/conformance JSON files successfully (`35` JSON files total).
- Inventory check found `8` e2e scenarios with all four required files.
- `cargo test --locked -p forge-domain` passed (`39` tests).
- `cargo test --locked -p forge-pipeline --lib` passed (`66` tests).
- `cargo test --locked -p forge-pipeline --test bypass_corpus` passed (`3` tests).
