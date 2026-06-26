# 033 Delete Legacy Codex Plugin And Devtools

## Slice goal

Delete the retired v0.4 Codex prompt docs, packaged plugin shell, and devtools OpenAPI stub while keeping the active `tools/codex-platform-mcp/` implementation and tests.

## Review mode

Independent Codex self-review. Claude Code review was intentionally not requested because the user instructed this run to proceed independently from Claude Code.

## Files changed

- `.agents/plugins/marketplace.json`
- `codex/`
- `codex-plugin/`
- `devtools/`
- `IMPLEMENTATION_STATUS.md`
- `docs/10_ACCEPTANCE_CHECKLIST.md`
- `native/*/README.md`
- `tools/check-repo.mjs`

## Deletion gate

The packaged plugin shell and devtools OpenAPI stub had no active runtime consumers. The only code consumers were `tools/check-repo.mjs` checks that validated the soon-to-be-deleted shell/stub. The active MCP implementation remains under `tools/codex-platform-mcp/`, and CI still runs that package's tests.

## Zero-reference proof

No live references to the deleted exact paths after removing the marketplace pointer:

```sh
rg -n --hidden -g '!.git/**' -g '!external-lib/**' -g '!forge/target/**' -g '!target/**' -g '!codex/**' -g '!codex-plugin/**' -g '!devtools/**' -g '!docs/**' -g '!review/**' -g '!review-from-claude/**' -g '!task-between-claude-and-codex/**' -g '!task-jun-15/**' "codex-plugin/platform-control|devtools/control-plane|CODEX_|plugin\\.mcp|control\\.openapi|checkPluginMcp|checkControlOpenApi" .
```

Generic active references to Codex workflows and `tools/codex-platform-mcp/` remain by design.

## Commands and evidence

- `node --test --no-warnings tools/codex-platform-mcp/test/tool-contract.test.js tools/codex-platform-mcp/test/server.test.js`
- `node --no-warnings tools/check-repo.mjs`
- `git diff --check`

## Findings

- `.agents/plugins/marketplace.json` still pointed at the deleted packaged plugin shell and was updated to an empty plugin list.
- Native README files still pointed at the deleted devtools OpenAPI stub and now point at `tools/codex-platform-mcp/README.md`.

## Resolution

- Deleted `codex/`, `codex-plugin/`, and `devtools/`.
- Removed stale `check-repo` validations for the deleted shell/stub.
- Kept the active MCP implementation and contract tests.
