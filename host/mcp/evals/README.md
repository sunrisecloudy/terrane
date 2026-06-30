# Terrane MCP Evals

These files are benchmark inputs for external agents. They are intentionally not
served by the Terrane MCP server.

Use them as user prompts for locked-down clients such as opencode agents with
source, filesystem, shell, web, and search tools denied. The model should solve
the task by discovering Terrane through MCP tools, resources, workflows, and
capability docs.

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
`app_scaffold` and finish with `length` before it reaches `app_register_inline`.

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

- `finish:length` before `app_register_inline`: output budget exhaustion. Retry
  with the opencode output-budget plugin, a smaller bundle, or a resume that
  registers the scaffolded files immediately.
- Provider stream error in `~/.local/share/opencode/log/opencode.log`: provider
  failure. Retry the same prompt/config before changing Terrane docs.
- No-token stall: opencode starts a new assistant stream, but the DB records a
  final/current assistant message with empty `finish`, `output = 0`, no new tool
  parts, and an unchanged `session.time_updated`. Stop the run and classify it
  as a client/provider stall unless Terrane logged a tool error.
- No app registered: `$TERRANE_HOME/apps/<id>` is absent. Check whether the model
  ever called `app_register_inline` and whether that call returned `isError`.
- Registered but unverified UI: app files exist and backend invokes work, but no
  browser/page check ran. Count this as an incomplete UI eval, not an MCP
  registration failure.

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
named explicitly.
