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

# Resume phase: build exit 124 (timeout) AND no installed app. Exit 0 with no
# app is a model result ("early stop"), not resumed — keep phases comparable.
while IFS=$'\t' read -r slug model label <&3; do
  [ -n "$slug" ] || continue
  build_exit="$(awk -F'\t' -v s="$slug" '$1==s && $3=="build" {print $4}' "$ROOT/results.tsv" | tail -1)"
  home="$ROOT/home-$slug"
  if [ "$build_exit" = "124" ] && [ -z "$(installed_app_id "$home")" ]; then
    "$harness_dir/resume-one.sh" "$model" "$slug" "$label" "$ROOT"
  fi
done 3< "$ROOT/models.tsv"

"$harness_dir/grade.sh" "$ROOT"
log ""
log "DONE root=$ROOT"
