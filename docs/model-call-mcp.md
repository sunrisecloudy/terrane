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

## Run The Committed Harness

The repeatable path is the harness under `host/mcp/evals/harness/`:

```sh
cd /Users/vehasuwat/Project/terrane
cargo build -p terrane-host-mcp
cargo build -p terrane-host-cli
cargo build -p terrane-host-web   # for browser verification
cd host/mcp/evals/harness && npm install   # optional: puppeteer-core UI check

./run-batch.sh                          # full batch: build -> resume -> grade
./run-batch.sh /private/tmp/my-root my-models.tsv
```

`run-batch.sh` runs every model in `models.tsv` sequentially (build phase),
gives eligible timeouts a resume phase, then grades everything into
`$ROOT/report.tsv` and `$ROOT/report.md`. Knobs are env vars resolved in
`lib.sh`: `BUILD_TIMEOUT` (8m), `RESUME_TIMEOUT` (4m), `PROMPT_FILE`,
`NL_QUERY`, `UI_INPUT_TEXT`, `EVAL_WEB_PORT`, and
`TERRANE_OPENCODE_MAX_OUTPUT_TOKENS`. Only the prompt and the two smoke
strings are task-specific — the harness itself is app-generic.

The sections below document what the harness sets up (still the contract under
test) plus the manual escape hatches.

## Preconditions

Build the MCP host:

```sh
cd /Users/vehasuwat/Project/terrane
cargo build -p terrane-host-mcp
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
        "/Users/vehasuwat/Project/terrane/target/debug/terrane-mcp"
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
3. `app_build_start`
4. `app_build_put_file` for any changed generated files
5. `app_build_validate`
6. `app_build_commit`
7. `app_actions`
8. `invoke`

The model may read MCP resources such as `terrane://docs/agent-playbook` or call
`capability_info`. That is allowed. It should not use repository file reads,
shell, broad filesystem listing, web fetch, or task tools.

The older `app_scaffold` + `app_register_inline` path remains a compatibility
bridge. After `app_scaffold`, the next model action should be
`app_register_inline` dry-run with the complete `structuredContent.files` array;
that dry-run returns `draftId` and `validationToken`, so the next action should
be `app_build_commit` without resending files. If the model writes a long
prose/code answer instead of calling a concrete tool, the docs are not sharp
enough for that model.

## Permission Handshake

Terrane resources are default-deny. The first `invoke` or `app_actions` call on
an app that uses `kv`, `crdt`, `relational_db`, or `build` may return
`isError: true` with `structuredContent.type == "permission_required"`.

That is not app failure. A trusted human/operator must approve the grant. Any of
these work; the last two apply to the **running** server (no restart):

- **In-session (elicitation):** if the model's client supports MCP elicitation,
  the server prompts the operator to approve right in the session and the model's
  `invoke` then succeeds on its own — often you see no `permission_required` at
  all. Set `TERRANE_ELICIT_TIMEOUT_MS` to bound the wait (default 120000).
- **Loopback admin console (live):** `curl -X POST
  http://127.0.0.1:8780/__terrane/admin/requests/<requestId>/approve` (or open
  `structuredContent.adminUrl`). Configurable via `TERRANE_ADMIN_ADDR`
  (`off` to disable). In parallel evals, always prefer the `adminUrl` returned
  in the permission object because the port may not be `8780`.
- **CLI grant:** `terrane auth grant user:local-owner <app> <namespace>` (one per
  `structuredContent.grantCommands` entry), then retry the same `invoke`.

**Single-writer lock:** while `terrane-mcp` (or `terrane-web`) is running
against a home, a second `terrane` process on that home is refused — the lock
is held for the server's whole lifetime. Ordering rule for grading: run **all
CLI grants and smoke commands before starting `terrane-web`**; once the server
is up, grant through the live admin console instead
(`POST /__terrane/admin/grants` with header `X-Terrane-Admin: local-admin` and
body `{"subject":"","app":"<id>","namespace":"kv"}`). A `terrane-mcp` child
that survives a `timeout` kill also holds the lock invisibly (TERRANE_HOME is
env, not argv) — find it with `lsof -t "$TERRANE_HOME"/*.lock`, which is what
the harness's `kill_home_holders` does after every phase.

## Resume Phase

A build run that exits `124` (timeout) **without** an installed app gets one
fresh 4-minute session in the same workdir and home, so `.mcp-drafts` drafts
are visible. Exit 0 with no app is a model result ("early stop") and is not
resumed. The resume message is `host/mcp/evals/prompts/resume-preamble.md`
plus the same product prompt: it says an **unfinished draft may still be
saved — don't start over**, in user-level words, and deliberately names no
tools; whether the model finds `app_build_list` is part of the test. Grading
records `resume_used`, `resume_ok` (app installed after resume), and
`resume_recovered` (the log shows `app_build_list` plus a reused `draft-*`
id, distinguishing recovery from a from-scratch rebuild).

## Browser Verification

For UI apps, backend smoke tests are not enough (run 2 shipped a frontend
args-array bug no CLI check could see). `grade.sh` serves each home with
`terrane-web --addr 127.0.0.1:$EVAL_WEB_PORT --no-live-reload` **after** the
CLI phase, then `grade-ui.mjs` (puppeteer-core + system Chrome; skipped with a
warning when missing) drives the shim-injected frame page
`/apps/<id>/__terrane/frame` and asserts, app-generically:

1. `GET /apps/<id>` returns 200.
2. `window.terrane.invoke` is a function on the frame page (the app's HTML/JS
   executed).
3. Zero uncaught page errors / `console.error` during load.
4. One interaction round-trip: type `UI_INPUT_TEXT` into the first text input,
   submit, and require both a 200 error-free `POST /apps/<id>/invoke` and a
   changed DOM within 10 seconds.

If the invoke returns `permission_required`, the grader grants over the live
admin console and retries once. Artifacts per model land in `$ROOT/ui-<slug>/`:
`page-load.png`, `after-interaction.png`, `console.log`, `network.log`,
`server.log`, `result.json`.

Recent weak-model runs showed that productive models sometimes tried
`capability_command auth.grant` after `permission_required`. Treat that as a
model mistake, not as a Terrane recovery path. The payload's
`operatorActionRequired`, `allowedMcpTools`, `forbiddenMcpTools`, and
`nextModelAction` fields are the contract: surface `grantCommands`/`adminUrl`,
poll `permission_check`, and retry the original call after trusted approval.

## Judge Success

`grade.sh` automates the checks below into `$ROOT/report.tsv` (one row per
model: installed, permission_stop_ok, self_grant_attempts, grant_ok,
backend_smoke, nl_query, ui_check, ui_args_array_warn, resume_*, tokens/cost)
and `$ROOT/report.md` (same plus output excerpts). The auto-verdicts are
triage; the rubric in `host/mcp/evals/rubrics/` stays the human authority.

Judge the produced app, not the transcript alone:

- The app exists under `$TERRANE_HOME/apps/<id>`.
- `list_apps` shows the app.
- `app_actions` returns useful verbs.
- `invoke` proves at least one write/read or app-specific workflow.
- Run a **domain-specific smoke check** that exercises the hardest requested
  behavior, not just build/commit. For the calendar task: seed events that
  cover the requested range, then run the exact natural-language query and
  check the matches are non-empty and correct.
- For UI apps, the coordinator opens the hosted page and verifies one visible
  user flow. Backend invoke success alone is not enough for a UI task. Check
  UI source for the positional `window.terrane.invoke("verb", "arg1", ...)`
  shape; an args-array call is a frontend bug backend smoke tests miss.
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

- `finish = "length"` before `app_build_validate` or `app_register_inline`:
  client/provider output budget. Retry with the output-budget plugin or a
  smaller first bundle.
- New assistant stream with `output = 0`, empty `finish`, no new tool parts, and
  unchanged `session.time_updated`: provider/client stall. Stop the run and
  restart from the last structured result. If the last completed call was
  `app_build_start`, call `app_build_get` or continue with `app_build_put_file`;
  if the `draftId` was lost, `app_build_list` recovers it. If the last call was
  `app_scaffold`, make `app_register_inline` dry-run the first resumed tool
  call. If it was `app_recipe`, `workflow_info`, or `capability_info`, resume
  with the next concrete tool named in `firstCalls`, `steps`,
  `nextAfterScaffold`, or `nextToolCall`. In a comparison batch, allow one total
  retry for this stall class; a repeated silent stall is a provider/client
  result, not evidence that Terrane docs are missing.
- `app_build_validate` or `app_register_inline` returns `isError: true` or
  `valid: false`: Terrane rejected the bundle. Validation now also enforces the
  JS runtime contract (no top-level `import`/`export`, a global
  `function handle(input)` or `actions` table) and prescriptive manifest shape
  errors (`ui` must be a string path). The errors carry fix-it guidance; a
  capable model should repair the named file and revalidate. A model that loops
  on the same validation error is a model result, not a docs gap.
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
