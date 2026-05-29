---
name: generated-webapp-repair
description: Repair a generated build-free webapp package by validating, installing, testing, patching HTML/CSS/vanilla JS, and retesting through the platform control MCP.
---

# Generated Webapp Repair

Use this skill when a generated app package fails validation, install gating, smoke tests, micro-tests, accessibility checks, or bridge/runtime assertions.

## Workflow

1. Preserve the package contract: `manifest.json`, `index.html`, `styles.css`, `app.js`, and recommended `smoke-tests.json`.
2. Run `platform.validate_package` and `platform.run_policy_audit` before editing so the first failure is grounded.
3. Patch only generated package files unless evidence points to runtime, host, MCP, or Zig core behavior.
4. Keep the app build-free: HTML, CSS, and vanilla JavaScript only.
5. Use documented bridge methods through `AppRuntime.call`; never use direct `fetch`, `localStorage`, cookies, IndexedDB, native globals, or `appId` request params.
6. Retest with package validation, smoke or micro-tests, and targeted assertions for the changed behavior.

## Repair Rules

- Preserve storage compatibility. If `dataVersion` changes, add consecutive migrations and expect approval-gated install behavior.
- Keep every interactive element testable with a stable `data-testid`.
- Take or use snapshots before destructive reset, migration, rollback, or storage repair.
- Check `runtime.bridge_calls`, `runtime.console_logs`, `runtime.snapshot`, and `db.export_debug_bundle` when a repair loop needs evidence.
- Update `docs/10_ACCEPTANCE_CHECKLIST.md` or `IMPLEMENTATION_STATUS.md` only when verification changes what is known.
