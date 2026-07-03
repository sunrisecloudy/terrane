#!/usr/bin/env bash
# Shared env + helpers for the Terrane weak-model MCP eval harness.
# Source this; do not execute it.

harness_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TERRANE_REPO="${TERRANE_REPO:-$(cd "$harness_dir/../../../.." && pwd)}"

MCP_BIN="${MCP_BIN:-$TERRANE_REPO/target/debug/terrane-mcp}"
CLI_BIN="${CLI_BIN:-$TERRANE_REPO/target/debug/terrane-host}"
WEB_BIN="${WEB_BIN:-$TERRANE_REPO/target/debug/terrane-web}"
OPENCODE_BIN="${OPENCODE_BIN:-opencode}"

# macOS ships neither; brew coreutils provides `timeout`, sometimes `gtimeout`.
if [ -z "${TIMEOUT_BIN:-}" ]; then
  if command -v timeout >/dev/null 2>&1; then TIMEOUT_BIN=timeout
  elif command -v gtimeout >/dev/null 2>&1; then TIMEOUT_BIN=gtimeout
  else
    echo "ERROR: need coreutils timeout or gtimeout on PATH" >&2
    exit 1
  fi
fi

BUILD_TIMEOUT="${BUILD_TIMEOUT:-8m}"
RESUME_TIMEOUT="${RESUME_TIMEOUT:-6m}"
TERRANE_OPENCODE_MAX_OUTPUT_TOKENS="${TERRANE_OPENCODE_MAX_OUTPUT_TOKENS:-131072}"
EVAL_WEB_PORT="${EVAL_WEB_PORT:-8790}"

# Domain knobs — the only calendar-specific pieces; override for other tasks.
PROMPT_FILE="${PROMPT_FILE:-$TERRANE_REPO/host/mcp/evals/prompts/calendar-app-outcome.md}"
RESUME_PREAMBLE="${RESUME_PREAMBLE:-$TERRANE_REPO/host/mcp/evals/prompts/resume-preamble.md}"
NL_QUERY="${NL_QUERY:-look at my events on saturdays over the last 5 months but show only the saturdays that have events}"
UI_INPUT_TEXT="${UI_INPUT_TEXT:-Dinner with Nok next Friday at 7pm at Siam Paragon}"
# One event guaranteed to match NL_QUERY — used to separate "thin seed data"
# from "broken query logic" when the first query returns zero.
NL_SEED_TEXT="${NL_SEED_TEXT:-Brunch with family last Saturday at 11am}"

log() {
  printf '%s\n' "$*" | tee -a "$ROOT/batch.log"
}

# results.tsv row: slug  model  phase  exit  workdir  home  log
results_append() {
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" "$7" >> "$ROOT/results.tsv"
}

# Kill any process still holding the home's single-writer lock. A terrane-mcp
# child can outlive `timeout`'s SIGTERM cascade; TERRANE_HOME lives in env,
# not argv, so pgrep -f cannot find it — lsof on the lock file can.
kill_home_holders() {
  local home="$1"
  local pids
  pids="$(lsof -t "$home"/*.lock 2>/dev/null || true)"
  if [ -n "$pids" ]; then
    echo "$pids" | xargs kill 2>/dev/null || true
    sleep 1
    pids="$(lsof -t "$home"/*.lock 2>/dev/null || true)"
    [ -n "$pids" ] && echo "$pids" | xargs kill -9 2>/dev/null || true
  fi
}

# First installed app id under a home, or empty.
installed_app_id() {
  local home="$1"
  local manifest
  for manifest in "$home"/apps/*/manifest.json; do
    [ -f "$manifest" ] || continue
    basename "$(dirname "$manifest")"
    return 0
  done
  echo ""
}
