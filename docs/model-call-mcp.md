# Calling Weak Models Through Opencode And Terrane MCP

This is the operator runbook for testing weaker or cheaper models against the
Terrane MCP surface without giving them repository source access. The goal is to
prove that a model can discover Terrane through MCP tools/resources and build an
app from the requested outcome.

Use this for models such as:

- `opencode-go/deepseek-v4-flash`
- `opencode-go/mimo-v2.5`
- `opencode-go/minimax-m3`

Do not put capability names, workflow names, or tool order in the product prompt
when the goal is blind discovery. Tell the model only the app outcome and proof
requirements.

## Preconditions

Build the MCP host:

```sh
cd /Users/vehasuwat/Project/terrane/host/mcp
cargo build
```

Use a throwaway home per run:

```sh
export TERRANE_REPO=/Users/vehasuwat/Project/terrane
export RUN_ID=calendar-dsv4-r1
export OPENCODE_WORK=/private/tmp/opencode-terrane-$RUN_ID
export TERRANE_HOME=/private/tmp/terrane-$RUN_ID-home
mkdir -p "$OPENCODE_WORK/.opencode/agent" "$TERRANE_HOME"
```

## Opencode Config

Create `$OPENCODE_WORK/.opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": [
    "/Users/vehasuwat/Project/terrane/host/mcp/evals/opencode/max-output-budget.mjs"
  ],
  "mcp": {
    "terrane": {
      "type": "local",
      "command": [
        "/Users/vehasuwat/Project/terrane/host/mcp/target/debug/terrane-mcp"
      ],
      "environment": {
        "TERRANE_HOME": "/private/tmp/terrane-calendar-dsv4-r1-home"
      },
      "enabled": true,
      "timeout": 30000
    }
  }
}
```

Set the `TERRANE_HOME` value to the run's actual throwaway home. Keep the
`max-output-budget.mjs` plugin for long code-generation turns; it asks opencode
to use the selected model's advertised output budget instead of a smaller client
default.

Optionally override the request budget:

```sh
export TERRANE_OPENCODE_MAX_OUTPUT_TOKENS=131072
```

## Locked Agent

Create `$OPENCODE_WORK/.opencode/agent/blind-local-app-builder.md`:

```md
---
description: Locked local app builder. Use only the configured local app-building surface; no filesystem, shell, web, source, or task tools.
mode: primary
permission:
  read: deny
  list: deny
  edit: deny
  write: deny
  glob: deny
  grep: deny
  bash: deny
  webfetch: deny
  task: deny
  todowrite: deny
  websearch: deny
  lsp: deny
  skill: deny
---

Use only the configured local app-building surface.
Do not use shell, file read/search/list/edit/write tools, web tools, task tools, or language-server tools.
```

The model should still have access to Terrane MCP tools, resources, and prompts
through the configured `terrane` MCP server.

## Run A Model

Use a product prompt file. For blind tests, the prompt should describe only the
user-facing app outcome, not the MCP route.

```sh
cd "$OPENCODE_WORK"
opencode run \
  "Build the app requested in the attached product prompt." \
  --agent blind-local-app-builder \
  --model opencode-go/deepseek-v4-flash \
  --title "Calendar app product request - DSV4 Flash" \
  --file /private/tmp/calendar-product-prompt.md
```

Swap the model to compare weaker models:

```sh
opencode run "Build the app requested in the attached product prompt." \
  --agent blind-local-app-builder \
  --model opencode-go/mimo-v2.5 \
  --title "Calendar app product request - MiMo V2.5" \
  --file /private/tmp/calendar-product-prompt.md
```

```sh
TERRANE_OPENCODE_MAX_OUTPUT_TOKENS=131072 \
opencode run "Build the app requested in the attached product prompt." \
  --agent blind-local-app-builder \
  --model opencode-go/minimax-m3 \
  --title "Calendar app product request - MiniMax M3" \
  --file /private/tmp/calendar-product-prompt.md
```

Remote opencode-go models receive the prompt and MCP tool/resource outputs.
Do not include secrets or private source text in eval prompts.

## Expected Model Route

A successful blind app-building run usually discovers this path:

1. `workflows_list`
2. `workflow_info`
3. `app_scaffold`
4. `app_register_inline` with `dryRun: true`
5. `app_register_inline` commit
6. `app_actions`
7. `invoke`

The model may read MCP resources such as `terrane://docs/agent-playbook` or call
`capability_info`. That is allowed. It should not use repository file reads,
shell, broad filesystem listing, web fetch, or task tools.

After `app_scaffold`, the next model action should be `app_register_inline`
dry-run with the complete `structuredContent.files` array. If the model writes a
long prose/code answer instead of calling the tool, the docs are not sharp
enough for that model.

## Permission Handshake

Terrane resources are default-deny. The first `invoke` or `app_actions` call on
an app that uses `kv`, `crdt`, `relational_db`, or `build` may return
`isError: true` with `structuredContent.type == "permission_required"`.

That is not app failure. A trusted human/operator must approve the grant:

```sh
terrane auth grant user:local-owner <app> <namespace>
```

Run each command from `structuredContent.grantCommands`, or approve through
`structuredContent.adminUrl`, then retry the same `invoke`.

## Judge Success

Judge the produced app, not the transcript alone:

- The app exists under `$TERRANE_HOME/apps/<id>`.
- `list_apps` shows the app.
- `app_actions` returns useful verbs.
- `invoke` proves at least one write/read or app-specific workflow.
- For UI apps, the coordinator opens the hosted page and verifies one visible
  user flow. Backend invoke success alone is not enough for a UI task.
- The opencode transcript shows no source reads, shell, broad filesystem list,
  web fetch, or task delegation.

## Diagnose Failed Runs

Opencode session DB:

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select id,title,tokens_input,tokens_output,tokens_reasoning,cost,time_updated
   from session
   where title like '%Calendar app product request%'
   order by time_created desc
   limit 5;"
```

Messages:

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select id,
          json_extract(data,'$.role'),
          json_extract(data,'$.finish'),
          json_extract(data,'$.tokens.input'),
          json_extract(data,'$.tokens.output'),
          json_extract(data,'$.tokens.reasoning'),
          time_updated
   from message
   where session_id='SESSION_ID'
   order by time_created,id;"
```

Tool parts:

```sh
sqlite3 ~/.local/share/opencode/opencode.db \
  "select json_extract(data,'$.type'),
          json_extract(data,'$.tool'),
          json_extract(data,'$.state.status'),
          length(json_extract(data,'$.text'))
   from part
   where session_id='SESSION_ID'
   order by time_created,id;"
```

Opencode log:

```sh
tail -n 200 ~/.local/share/opencode/log/opencode.log
```

Failure classes:

- `finish = "length"` before `app_register_inline`: client/provider output
  budget. Retry with the output-budget plugin or a smaller first bundle.
- New assistant stream with `output = 0`, empty `finish`, no new tool parts, and
  unchanged `session.time_updated`: provider/client stall. Stop the run and
  restart from the scaffold result.
- `app_register_inline` returns `isError: true`: Terrane rejected the bundle.
  Fix the complete files array and retry dry-run.
- No `$TERRANE_HOME/apps/<id>` directory: the model never committed registration
  or registration failed.
- App exists but UI was not opened: incomplete UI eval, not an MCP failure.

A practical watchdog is five minutes with no DB/message/tool progress after a
new assistant stream. Stop the run, record the last completed tool call, and name
the missing next tool call.

## Related Docs

- `host/mcp/docs/CLIENTS.md`
- `host/mcp/docs/AGENT_PLAYBOOK.md`
- `host/mcp/evals/README.md`
- `host/mcp/evals/opencode/max-output-budget.mjs`
