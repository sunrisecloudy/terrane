#!/usr/bin/env bash
# One model, build phase: locked opencode agent against the Terrane MCP server
# in an isolated home. Usage: run-one.sh MODEL SLUG LABEL ROOT
set -u
source "$(dirname "$0")/lib.sh"

model="$1"; slug="$2"; label="$3"; ROOT="$4"
TAG="$(basename "$ROOT")"
work="$ROOT/opencode-$slug"
home="$ROOT/home-$slug"
prompt="$ROOT/prompt-$slug.md"
out="$ROOT/out-$slug.log"
mkdir -p "$work/.opencode/agent" "$home"

python3 - "$PROMPT_FILE" "$prompt" "$slug" "$label" <<'PY'
import sys
src, dst, slug, label = sys.argv[1:]
text = open(src, encoding='utf-8').read()
text = text.replace('{{APP_ID}}', f'calendar-{slug}')
text = text.replace('{{APP_NAME}}', f'Calendar {label}')
open(dst, 'w', encoding='utf-8').write(text)
PY

cat > "$work/.opencode/opencode.json" <<JSON
{
  "\$schema": "https://opencode.ai/config.json",
  "plugin": ["$TERRANE_REPO/host/mcp/evals/opencode/max-output-budget.mjs"],
  "mcp": {
    "terrane": {
      "type": "local",
      "command": ["$MCP_BIN"],
      "environment": {
        "TERRANE_HOME": "$home",
        "TERRANE_ELICIT_TIMEOUT_MS": "120000"
      },
      "enabled": true,
      "timeout": 30000
    }
  }
}
JSON

cat > "$work/.opencode/agent/blind-local-app-builder.md" <<'MD'
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
MD

log ""
log "=== build: $label ($model) ==="
(
  cd "$work" || exit 88
  TERRANE_OPENCODE_MAX_OUTPUT_TOKENS="$TERRANE_OPENCODE_MAX_OUTPUT_TOKENS" \
    "$TIMEOUT_BIN" "$BUILD_TIMEOUT" "$OPENCODE_BIN" run \
    "Build the app requested in the attached product prompt." \
    --agent blind-local-app-builder \
    --model "opencode-go/$model" \
    --title "Terrane eval $TAG - $label - $slug - build" \
    --file "$prompt" \
    </dev/null
) >"$out" 2>&1
status=$?
kill_home_holders "$home"
results_append "$slug" "$model" build "$status" "$work" "$home" "$out"
log "build status=$status home=$home"
exit 0
