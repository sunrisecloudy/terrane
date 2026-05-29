---
name: platform-micro-test
description: Run generated webapp micro-tests, smoke tests, or platform smoke suites through the Native AI Webapp Platform control MCP without bypassing the runtime bridge.
---

# Platform Micro-Test

Use this skill when the user asks to validate, install, open, smoke-test, or micro-test a generated webapp package on a dev host or fake host.

## Workflow

1. Confirm the target host with `platform.health` and `platform.list_targets` when the target is not explicit.
2. Validate the package with `platform.validate_package`; stop on policy or schema failures.
3. Install with `platform.install_webapp_package`, then open with `platform.open_webapp`.
4. Run the smallest relevant test first: `runtime.run_microtest`, `runtime.run_smoke_tests`, or `platform.run_platform_smoke`.
5. Inspect failures with `runtime.snapshot`, `runtime.console_logs`, `runtime.bridge_calls`, `runtime.event_log`, and `runtime.assert_no_console_errors`.
6. Report the exact failing assertion, bridge method, app id, and host target.

## Guardrails

- Never call generated app APIs directly; use MCP tools only.
- Treat `data-testid` as the primary UI selector.
- Do not use raw SQL; DB inspection goes through `db.snapshot` and fixed `db.query_*` tools.
- Destructive controls must include `confirm: true` and should only be used at an explicit repair or reset boundary.
- If a package or host surface changed, run the relevant verification and compare `docs/10_ACCEPTANCE_CHECKLIST.md` plus `IMPLEMENTATION_STATUS.md`.
