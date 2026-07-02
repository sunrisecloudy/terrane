#!/usr/bin/env bash
# Whole eval batch: build phase per model, resume phase for eligible timeouts,
# then automated grading. Usage: run-batch.sh [ROOT] [MODELS_TSV]
set -u
source "$(dirname "$0")/lib.sh"

TAG="${TAG:-terrane-model-eval-$(date +%Y%m%d-%H%M%S)}"
ROOT="${1:-/private/tmp/$TAG}"
models_tsv="${2:-$harness_dir/models.tsv}"
mkdir -p "$ROOT"
cp "$models_tsv" "$ROOT/models.tsv"
: > "$ROOT/results.tsv"

log "batch root: $ROOT"
log "prompt: $PROMPT_FILE"

# Build phase (sequential). fd 3 keeps the loop's stdin away from children.
while IFS=$'\t' read -r slug model label <&3; do
  [ -n "$slug" ] || continue
  "$harness_dir/run-one.sh" "$model" "$slug" "$label" "$ROOT"
done 3< "$ROOT/models.tsv"

# Resume phase: any build that ended without an installed app gets one short
# second session. Run-4 showed provider stalls also end with exit 0 and a few
# hundred output tokens, so exit code alone cannot separate "model early
# stop" from "provider died" — the taxonomy's one-retry rule covers both, and
# report.tsv records resume_used so the phases stay distinguishable.
while IFS=$'\t' read -r slug model label <&3; do
  [ -n "$slug" ] || continue
  home="$ROOT/home-$slug"
  if [ -f "$ROOT/out-$slug.log" ] && [ -z "$(installed_app_id "$home")" ]; then
    "$harness_dir/resume-one.sh" "$model" "$slug" "$label" "$ROOT"
  fi
done 3< "$ROOT/models.tsv"

"$harness_dir/grade.sh" "$ROOT"
log ""
log "DONE root=$ROOT"
