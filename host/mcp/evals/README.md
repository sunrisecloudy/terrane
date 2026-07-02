# Terrane MCP Evals

These files are benchmark inputs for external agents. They are intentionally not
served by the Terrane MCP server.

Use them as user prompts for locked-down clients such as opencode agents with
source, filesystem, shell, web, and search tools denied. The model should solve
the task by discovering Terrane through MCP tools, resources, workflows, and
capability docs.

## Harness

`harness/` holds the committed runner and grader (see `docs/model-call-mcp.md`
for the operator runbook):

| Script | Role |
| --- | --- |
| `run-batch.sh [ROOT] [MODELS_TSV]` | whole batch: build → resume → grade |
| `run-one.sh MODEL SLUG LABEL ROOT` | one model, build phase (8m default) |
| `resume-one.sh MODEL SLUG LABEL ROOT` | one model, resume phase (4m default) |
| `grade.sh ROOT` | automated grading → `report.tsv` + `report.md` |
| `serve-ui.sh start/stop` | terrane-web lifecycle for the UI phase |
| `grade-ui.mjs` | headless browser check (optional `npm install`) |
| `lib.sh` | shared env knobs and lock cleanup |

Env knobs: `BUILD_TIMEOUT`, `RESUME_TIMEOUT`, `PROMPT_FILE`, `NL_QUERY`,
`UI_INPUT_TEXT`, `EVAL_WEB_PORT`, `TERRANE_OPENCODE_MAX_OUTPUT_TOKENS`,
`MCP_BIN`/`CLI_BIN`/`WEB_BIN`/`OPENCODE_BIN`/`TIMEOUT_BIN`.

`results.tsv` rows are `slug model phase exit workdir home log` (phases:
`build`, `resume`). `report.tsv` columns: installed, app_id,
permission_stop_ok, self_grant_attempts, grant_ok, backend_smoke, nl_query
(`pass|zero|error|absent`), ui_check (`pass|fail|skipped|needs_grant|no_ui|
server_failed`), ui_args_array_warn, resume_used/resume_ok/resume_recovered,
tokens, cost. `resume_recovered` means the resume log shows `app_build_list`
plus a reused `draft-*` id — recovery, not a from-scratch rebuild.

`harness/node_modules` is optional and git-ignored; only `package.json` is
committed. Without it (or without a system Chrome) the UI check records
`skipped` and grading still completes.

## Eval Prompt Rules

- Eval prompts are not MCP resources or MCP prompts.
- Eval prompts describe outcomes, constraints, and proof requirements.
- Eval prompts should not name the exact workflow, scaffold kind, or tool order.
- Expected success signals belong in the prompt metadata so runs can be compared
  across models without changing the server under test.

## App-Only Prompts

Some prompts intentionally look like ordinary product requests. For those, send
only the prompt file to the model; keep any matching rubric file for the
coordinator. The model should not be told that the run is an evaluation, and the
coordinator should judge from the produced app behavior rather than the model's
route or transcript.

## Opencode Output Budget

Locked opencode runs should not use the default 32k response budget for weak
models. A model can spend that whole budget drafting a bundle after
`app_build_start` or `app_scaffold` and finish with `length` before it reaches
`app_build_validate` or the compatibility `app_register_inline` dry-run.

Add the eval plugin to the temporary opencode workspace:

```json
{
  "plugin": [
    "/absolute/path/to/terrane/host/mcp/evals/opencode/max-output-budget.mjs"
  ],
  "mcp": {
    "terrane": {
      "type": "local",
      "command": ["/absolute/path/to/terrane/host/mcp/target/debug/terrane-mcp"],
      "environment": {
        "TERRANE_HOME": "/private/tmp/terrane-eval-home"
      },
      "enabled": true,
      "timeout": 30000
    }
  }
}
```

The plugin sets `maxOutputTokens` to the selected model's advertised
`limit.output`. Set `TERRANE_OPENCODE_MAX_OUTPUT_TOKENS` in the eval environment
only when you intentionally want a smaller or larger request budget supported by
the provider.

If a run exits without registering an app, check opencode's session database for
the final assistant message. A `finish` value of `length` means the model hit the
client/provider output budget; it is not a Terrane MCP failure.

## Failure Taxonomy

Classify failed runs before updating prompts, tools, or docs:

- `finish:length` before `app_build_validate` or `app_register_inline`: output
  budget exhaustion. Retry with the opencode output-budget plugin, a smaller
  bundle, or a resume that validates/commits the current draft immediately.
- Provider stream error in `~/.local/share/opencode/log/opencode.log`: provider
  failure. Retry the same prompt/config before changing Terrane docs.
- No-token stall: opencode starts a new assistant stream, but the DB records a
  final/current assistant message with empty `finish`, `output = 0`, no new tool
  parts, and an unchanged `session.time_updated`. Stop the run and classify it
  as a client/provider stall unless Terrane logged a tool error.
- No-token stall after `app_recipe`, `workflow_info`, `capability_info`,
  `app_build_start`, or `app_scaffold`: resume from the last structured tool
  result with the next concrete tool call named there. Do not ask the model for
  a prose summary. For app tasks this is usually `app_build_start`,
  `app_build_get`/`app_build_put_file`, `app_build_validate`, or the
  compatibility `app_register_inline` dry-run if `structuredContent.files`
  already exists. Allow at most one retry for this class in a comparison batch;
  if it stalls again, record provider/client stall and move on.
- Self-grant attempt after `permission_required`: the model called
  `capability_command` with `auth.*`, `*.grant`, or similar after a resource
  denial. Count this as a docs/tool-guidance miss. The correct model behavior is
  to surface `grantCommands`/`adminUrl`, poll `permission_check`, and retry the
  original call after trusted approval.
- No app registered: `$TERRANE_HOME/apps/<id>` is absent. Check whether the model
  ever called `app_build_commit` or `app_register_inline`, and whether the
  preceding validation/dry-run returned `isError`.
- Registered but unverified UI: app files exist and backend invokes work, but no
  browser/page check ran. Count this as an incomplete UI eval, not an MCP
  registration failure.

## Six-Model Lessons

The July 2026 calendar-app batch across DeepSeek V4 Flash, MiMo V2.5,
MiniMax M3, DeepSeek V4 Pro, GLM 5.2, and Kimi K2.7 Code produced two useful
MCP findings:

- Productive weak models can create usable app bundles and backend flows from
  MCP docs alone, but they may hit `permission_required` and try to self-grant
  with `capability_command auth.grant`. Improve the denial payload and docs, not
  the eval prompt.
- Several models stalled with zero output tokens immediately after
  `app_build_start`, `app_scaffold`, `app_recipe`, or `capability_info`. Treat this as a
  client/provider stall unless Terrane returned a tool error, and make the last
  tool result carry an explicit `nextToolCall`/`nextModelAction`. One retry is
  enough to distinguish a transient provider hiccup from a repeatable stall.
- Requiring models to resend a whole `files` array after dry-run is fragile.
  Prefer the staged draft tools and make the compatibility inline dry-run return
  `draftId`/`validationToken` for `app_build_commit`.

When updating MCP docs after such runs, prefer machine-readable fields in tool
results (`nextToolCall`, `nextAfterScaffold`, `operatorActionRequired`,
`allowedMcpTools`, `forbiddenMcpTools`, `nextModelAction`) over longer prose.

The staged run of the same calendar batch added a third finding: weak models no
longer fail at discovery, they fail at **contract precision** after discovery —
Deno/Node module backends that validated but could not run, `input.action`
object dispatch instead of positional `input[0]`, and object-shaped
`manifest.ui`. The fixes shipped after that run move the contracts into the
closest surface: the `initialize` result's `instructions` field, an inline
`contract` object in `app_build_start`, JS-contract and manifest-shape
enforcement with fix-it errors in `app_build_validate`, and `app_build_list`
for lost-draftId stall recovery.

## Watchdog Checks

For long locked-agent runs, keep a small watchdog loop outside the model:

1. Record the opencode session id and `TERRANE_HOME`.
2. Poll `session.tokens_output`, `session.time_updated`, message finish reasons,
   and tool parts from `~/.local/share/opencode/opencode.db`.
3. Inspect `~/.local/share/opencode/log/opencode.log` for provider stream
   errors.
4. Check whether `$TERRANE_HOME/apps/<id>` exists before judging success.

A practical default is to stop a run after five minutes with no DB/message/tool
progress following a new assistant stream. The result should be recorded as a
no-token stall, with the last completed tool call and the missing next tool call
named explicitly. In a six-model batch, keep a single global retry slot for the
first silent stall so flaky providers do not dominate the run.
