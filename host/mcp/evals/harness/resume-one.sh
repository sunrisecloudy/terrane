#!/usr/bin/env bash
# One model, resume phase: a fresh short session in the SAME workdir/home so
# server-side drafts are visible. The preamble mentions an unfinished draft in
# user-level words but never tool names — recovery routing stays a discovery
# test. Usage: resume-one.sh MODEL SLUG LABEL ROOT
set -u
source "$(dirname "$0")/lib.sh"

model="$1"; slug="$2"; label="$3"; ROOT="$4"
TAG="$(basename "$ROOT")"
work="$ROOT/opencode-$slug"
home="$ROOT/home-$slug"
prompt="$ROOT/prompt-$slug.md"
out="$ROOT/out-$slug-resume.log"

if [ ! -d "$work" ] || [ ! -f "$prompt" ]; then
  log "resume: skipping $slug (no build workdir/prompt)"
  exit 0
fi

log ""
log "=== resume: $label ($model) ==="
(
  cd "$work" || exit 88
  TERRANE_OPENCODE_MAX_OUTPUT_TOKENS="$TERRANE_OPENCODE_MAX_OUTPUT_TOKENS" \
    "$TIMEOUT_BIN" "$RESUME_TIMEOUT" "$OPENCODE_BIN" run \
    "$(cat "$RESUME_PREAMBLE")" \
    --agent blind-local-app-builder \
    --model "opencode-go/$model" \
    --title "Terrane eval $TAG - $label - $slug - resume" \
    --file "$prompt" \
    </dev/null
) >"$out" 2>&1
status=$?
kill_home_holders "$home"
results_append "$slug" "$model" resume "$status" "$work" "$home" "$out"
log "resume status=$status home=$home"
exit 0
