#!/usr/bin/env bash
# Automated grading for one eval batch: per model, CLI-phase checks (install,
# permission behavior, grants, backend smoke, NL query), then a browser UI
# check via terrane-web + grade-ui.mjs. Writes report.tsv and report.md.
#
# Lock ordering matters: every CLI command takes the home's single-writer
# lock, so ALL CLI grading runs before terrane-web starts; once the server is
# up, grants go through the live admin console instead.
# Usage: grade.sh ROOT
set -u
source "$(dirname "$0")/lib.sh"

ROOT="$1"
TAG="$(basename "$ROOT")"
report_tsv="$ROOT/report.tsv"
report_md="$ROOT/report.md"
printf 'slug\tmodel\tinstalled\tapp_id\tpermission_stop_ok\tself_grant_attempts\tgrant_ok\tbackend_smoke\tnl_query\tui_check\tui_args_array_warn\tresume_used\tresume_ok\tresume_recovered\ttokens_in\ttokens_out\tcost\n' > "$report_tsv"
printf '# Eval report: %s\n\n' "$TAG" > "$report_md"

run_verb() { # home app verb [arg]
  local home="$1" app="$2" verb="$3"
  shift 3
  env TERRANE_HOME="$home" "$CLI_BIN" run --permission-ui none "$app" "$verb" "$@" 2>&1 | tail -1
}

# Classify an NL query reply: pass (results present), zero, error, or absent.
classify_output() {
  python3 - "$1" <<'PY'
import json, re, sys
out = sys.argv[1].strip()
if not out:
    print("error"); sys.exit()
if re.search(r"runtime error|unknown verb|error:", out, re.I) and not out.startswith("{") and not out.startswith("["):
    print("error"); sys.exit()
try:
    data = json.loads(out)
except Exception:
    print("pass" if len(out) > 0 else "zero"); sys.exit()
# Count only result-shaped content: explicit count keys and arrays under
# result-ish keys. Filter metadata (daysOfWeek etc.) must not count.
COUNT_KEYS = {"count", "total", "totalEvents", "matchedCount", "matched_count"}
RESULT_KEYS = {"events", "results", "items", "groups", "matches", "entries", "rows"}
def leaf(d):
    return not any(k in RESULT_KEYS and isinstance(v, list) for k, v in d.items())
def count_item(x):
    if isinstance(x, dict):
        return 1 if leaf(x) else count(x)
    if isinstance(x, list):
        return sum(count_item(y) for y in x)
    return 1
def count(node):
    if isinstance(node, list):
        return sum(count_item(x) for x in node)
    if isinstance(node, dict):
        explicit = max(
            (v for k, v in node.items() if k in COUNT_KEYS and isinstance(v, int)),
            default=0,
        )
        structural = sum(
            sum(count_item(x) for x in v)
            for k, v in node.items()
            if k in RESULT_KEYS and isinstance(v, list)
        )
        nested = sum(count(v) for v in node.values() if isinstance(v, dict))
        return max(explicit, structural + nested)
    return 0
if isinstance(data, dict) and str(data.get("ok")).lower() == "false":
    print("error"); sys.exit()
print("pass" if count(data) > 0 else "zero")
PY
}

while IFS=$'\t' read -r slug model label <&3; do
  [ -n "$slug" ] || continue
  home="$ROOT/home-$slug"
  build_log="$ROOT/out-$slug.log"
  resume_log="$ROOT/out-$slug-resume.log"
  ui_dir="$ROOT/ui-$slug"
  mkdir -p "$ui_dir"

  app_id="$(installed_app_id "$home")"
  installed=$([ -n "$app_id" ] && echo yes || echo no)

  permission_stop_ok=no
  if cat "$build_log" "$resume_log" 2>/dev/null | grep -q "permission_required"; then
    permission_stop_ok=yes
  fi
  self_grant_attempts="$(cat "$build_log" "$resume_log" 2>/dev/null | \
    grep -cE 'terrane_capability_command \{"name":"(auth\.[a-z._]+|[a-z._]+\.grant)' || true)"

  resume_used=no; resume_ok=no; resume_recovered=no
  if [ -f "$resume_log" ]; then
    resume_used=yes
    [ -n "$app_id" ] && resume_ok=yes
    if grep -q "terrane_app_build_list" "$resume_log" && grep -q "draft-" "$resume_log"; then
      resume_recovered=yes
    fi
  fi

  grant_ok=n/a; backend_smoke=n/a; nl_query=n/a; ui_check=n/a; ui_args_array_warn=no
  actions_json=""; seed_out=""; nl_out=""
  if [ -n "$app_id" ]; then
    # --- CLI phase (before terrane-web; the CLI needs the home lock) ---
    grant_ok=yes
    resources="$(python3 -c "import json,sys;print(' '.join(json.load(open(sys.argv[1])).get('resources',[])))" "$home/apps/$app_id/manifest.json" 2>/dev/null || echo kv)"
    for ns in $resources; do
      if ! env TERRANE_HOME="$home" "$CLI_BIN" auth grant user:local-owner "$app_id" "$ns" >/dev/null 2>&1; then
        grant_ok=no
      fi
    done

    actions_json="$(run_verb "$home" "$app_id" __actions__)"
    verbs="$(python3 -c "
import json, sys
try:
    data = json.loads(sys.argv[1])
    print(' '.join(a.get('verb','') for a in data.get('actions', [])))
except Exception:
    print('')
" "$actions_json" 2>/dev/null)"
    # Models sometimes drop __actions__ when replacing main.js — fall back to
    # grepping the dispatcher (case 'verb': / verb === 'verb') for verb names.
    if [ -z "$(echo "$verbs" | tr -d ' ')" ]; then
      verbs="$(grep -ohE "case ['\"][a-zA-Z_]+['\"]|verb *===? *['\"][a-zA-Z_]+['\"]" \
        "$home/apps/$app_id"/*.js 2>/dev/null | \
        grep -oE "['\"][a-zA-Z_]+['\"]" | tr -d "\"'" | sort -u | tr '\n' ' ')"
    fi

    seed_verb="$(echo "$verbs" | tr ' ' '\n' | grep -iE 'seed|sample|init|demo' | head -1)"
    if [ -n "$seed_verb" ]; then
      seed_out="$(run_verb "$home" "$app_id" "$seed_verb")"
      case "$seed_out" in
        *"runtime error"*|*"error"*) backend_smoke="error" ;;
        "") backend_smoke="empty" ;;
        *) backend_smoke="ok" ;;
      esac
    else
      backend_smoke="absent"
    fi

    # Try up to three query-shaped verbs and keep the best classification —
    # apps often expose a parse-only verb (returns the interpreted filter,
    # zero results) next to the real query verb.
    nl_query="absent"; nl_out=""
    nl_candidates="$(echo "$verbs" | tr ' ' '\n' | grep -ivE 'parse' | grep -iE 'natural|nl_|nlq|query|view|search|ask' | head -3)
$(echo "$verbs" | tr ' ' '\n' | grep -iE 'parse' | grep -iE 'view|query|natural|nl' | head -1)"
    nl_verb_used=""
    for candidate in $nl_candidates; do
      out="$(run_verb "$home" "$app_id" "$candidate" "$NL_QUERY")"
      verdict="$(classify_output "$out")"
      case "$nl_query" in
        pass) break ;;
        absent|error) nl_query="$verdict"; nl_out="$out"; nl_verb_used="$candidate" ;;
        zero) if [ "$verdict" = "pass" ]; then nl_query="$verdict"; nl_out="$out"; nl_verb_used="$candidate"; fi ;;
      esac
      [ "$nl_query" = "pass" ] && break
    done

    # A "zero" can mean thin seed data rather than broken query logic. Prove
    # it: add one event that matches the query, re-run it, and record
    # pass_after_add when the new event shows up.
    if [ "$nl_query" = "zero" ] && [ -n "$nl_verb_used" ]; then
      add_verb="$(echo "$verbs" | tr ' ' '\n' | grep -iE 'nl_event|nl_create|nlcreate|parse_event|parseevent|add|create|new' | grep -ivE 'view|query' | head -1)"
      if [ -n "$add_verb" ]; then
        run_verb "$home" "$app_id" "$add_verb" "$NL_SEED_TEXT" >/dev/null 2>&1
        out="$(run_verb "$home" "$app_id" "$nl_verb_used" "$NL_QUERY")"
        if [ "$(classify_output "$out")" = "pass" ]; then
          nl_query="pass_after_add"
          nl_out="$out"
        fi
      fi
    fi

    # Static run-1 lesson: args array passed to the invoke bridge.
    if grep -rE 'invoke\([^)]*, *\[' "$home/apps/$app_id" --include='*.js' --include='*.html' >/dev/null 2>&1; then
      ui_args_array_warn=yes
    fi

    # --- UI phase (server holds the home lock while it runs) ---
    has_ui="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('ui',''))" "$home/apps/$app_id/manifest.json" 2>/dev/null)"
    if [ -n "$has_ui" ]; then
      kill_home_holders "$home"
      if "$harness_dir/serve-ui.sh" start "$home" "$EVAL_WEB_PORT" "$ui_dir" >/dev/null; then
        base="http://127.0.0.1:$EVAL_WEB_PORT"
        node "$harness_dir/grade-ui.mjs" "$base" "$app_id" "$ui_dir" "$UI_INPUT_TEXT" >>"$ui_dir/grade-ui.out" 2>>"$ui_dir/grade-ui.err"
        verdict="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('verdict','fail'))" "$ui_dir/result.json" 2>/dev/null || echo error)"
        if [ "$verdict" = "needs_grant" ]; then
          # Late grant over the live admin console, then one retry.
          ns_list="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('needs_grant') or 'kv')" "$ui_dir/result.json" 2>/dev/null)"
          for ns in $(echo "$ns_list" | tr ',' ' '); do
            curl -sf -X POST -H "X-Terrane-Admin: local-admin" -H "Content-Type: application/json" \
              -d "{\"subject\":\"\",\"app\":\"$app_id\",\"namespace\":\"$ns\"}" \
              "$base/__terrane/admin/grants" >/dev/null || true
          done
          node "$harness_dir/grade-ui.mjs" "$base" "$app_id" "$ui_dir" "$UI_INPUT_TEXT" >/dev/null 2>>"$ui_dir/grade-ui.err"
          verdict="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('verdict','fail'))" "$ui_dir/result.json" 2>/dev/null || echo fail)"
        fi
        ui_check="$verdict"
        "$harness_dir/serve-ui.sh" stop "$ui_dir" "$home" "$EVAL_WEB_PORT"
      else
        ui_check="server_failed"
      fi
    else
      ui_check="no_ui"
    fi
  fi

  # Session totals can lag or the DB can be briefly locked right after a run;
  # retry once before accepting zeros.
  tokens_query() {
    sqlite3 -tabs ~/.local/share/opencode/opencode.db \
      "select coalesce(sum(tokens_input),0), coalesce(sum(tokens_output),0), coalesce(sum(cost),0)
       from session where title like 'Terrane eval $TAG - % - $slug - %';" 2>/dev/null || printf '0\t0\t0'
  }
  tokens="$(tokens_query)"
  if [ "$(echo "$tokens" | cut -f1)" = "0" ]; then
    sleep 5
    tokens="$(tokens_query)"
  fi
  tokens_in="$(echo "$tokens" | cut -f1)"
  tokens_out="$(echo "$tokens" | cut -f2)"
  cost="$(echo "$tokens" | cut -f3)"

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$slug" "$model" "$installed" "$app_id" "$permission_stop_ok" "$self_grant_attempts" \
    "$grant_ok" "$backend_smoke" "$nl_query" "$ui_check" "$ui_args_array_warn" \
    "$resume_used" "$resume_ok" "$resume_recovered" "$tokens_in" "$tokens_out" "$cost" >> "$report_tsv"

  {
    printf '## %s (%s)\n\n' "$label" "$model"
    printf -- '- installed: %s (app_id: %s)\n' "$installed" "${app_id:-—}"
    printf -- '- permission_stop_ok: %s, self_grant_attempts: %s\n' "$permission_stop_ok" "$self_grant_attempts"
    printf -- '- resume: used=%s ok=%s recovered=%s\n' "$resume_used" "$resume_ok" "$resume_recovered"
    printf -- '- backend_smoke: %s\n' "$backend_smoke"
    [ -n "$seed_out" ] && printf '  - seed output: `%s`\n' "$(echo "$seed_out" | head -c 200)"
    printf -- '- nl_query: %s\n' "$nl_query"
    [ -n "$nl_out" ] && printf '  - query output: `%s`\n' "$(echo "$nl_out" | head -c 300)"
    printf -- '- ui_check: %s, ui_args_array_warn: %s (artifacts: ui-%s/)\n' "$ui_check" "$ui_args_array_warn" "$slug"
    printf -- '- tokens: in=%s out=%s cost=%s\n\n' "$tokens_in" "$tokens_out" "$cost"
  } >> "$report_md"
done 3< "$ROOT/models.tsv"

log "report: $report_tsv"
log "report: $report_md"
