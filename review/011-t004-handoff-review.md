# 011 - T004 Handoff Review

Reviewed commit: `28d70d0` (`collab: delegate T004 (static-scan bypass corpus) to Codex`)

## Findings

- No blocking issues in the handoff commit. The request is scoped, actionable, and directly addresses review 010's P1 static-scan bypass risk.

## Help Delivered

- Added `forge/crates/pipeline/tests/bypass/` with 23 rejected bypass fixtures and 4 benign controls.
- Added `forge/crates/pipeline/tests/bypass/manifest.json` with `technique`, `target`, `expect`, and `reason` metadata for each case.
- Updated `task-between-claude-and-codex/T004-scan-bypass-corpus.md` to `status: completed` with a `## Result` section separating alias/data-flow cases from pure AST/member-check cases.

## Verification

- `node -e ...` manifest/file-list check passed (`27 cases ok`).
- `cargo test --locked -p forge-pipeline` passed.

## Follow-Up

- Once the scanner hardening lands, wire `tests/bypass/manifest.json` into a real integration test so rejected cases must fail and benign controls must pass in CI.
