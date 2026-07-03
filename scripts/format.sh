#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK=0

if [[ "${1:-}" == "--check" ]]; then
  CHECK=1
  shift
elif [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<'USAGE'
usage: scripts/format.sh [--check]

Formats active Terrane source files:
  - Root Rust workspace with cargo fmt
  - JS/JSX/MJS/TS/TSX/JSON/HTML/CSS/Markdown/YAML with deno fmt
  - macOS Swift sources and tests with swift format

Generated dist files, vendored assets, build outputs, local state, and target
dirs are intentionally skipped.
USAGE
  exit 0
fi

log() {
  printf '\n==> %s\n' "$*"
}

run_in() {
  local dir="$1"
  shift
  local display="${dir#$ROOT/}"
  [[ "$dir" == "$ROOT" ]] && display="."
  log "(cd $display && $*)"
  (cd "$dir" && "$@")
}

if [[ "$CHECK" -eq 1 ]]; then
  run_in "$ROOT" cargo fmt --all --check
else
  run_in "$ROOT" cargo fmt --all
fi

deno_files=()
while IFS= read -r file; do
  deno_files+=("$file")
done < <(
  find "$ROOT" \
    -path "$ROOT/.agents" -prune -o \
    -path "$ROOT/.claude" -prune -o \
    -path "$ROOT/.forge-wf" -prune -o \
    -path "$ROOT/.git" -prune -o \
    -path "$ROOT/.terrane" -prune -o \
    -path '*/target/*' -prune -o \
    -path '*/dist/*' -prune -o \
    -path '*/vendor/*' -prune -o \
    -path '*/.derived/*' -prune -o \
    -path '*/build/*' -prune -o \
    -path '*/node_modules/*' -prune -o \
    \( \
      -name '*.js' -o \
      -name '*.jsx' -o \
      -name '*.mjs' -o \
      -name '*.ts' -o \
      -name '*.tsx' -o \
      -name '*.json' -o \
      -name '*.html' -o \
      -name '*.css' -o \
      -name '*.md' -o \
      -name '*.yml' -o \
      -name '*.yaml' \
    \) -print \
    | sort
)

if [[ "${#deno_files[@]}" -gt 0 ]]; then
  if [[ "$CHECK" -eq 1 ]]; then
    log "deno fmt --check (${#deno_files[@]} files)"
    deno fmt --check --no-config "${deno_files[@]}"
  else
    log "deno fmt (${#deno_files[@]} files)"
    deno fmt --no-config "${deno_files[@]}"
  fi
fi

if swift format --version >/dev/null 2>&1; then
  if [[ "$CHECK" -eq 1 ]]; then
    log "swift format lint host/macos/Sources host/macos/Tests"
    swift format lint --strict --recursive "$ROOT/host/macos/Sources" "$ROOT/host/macos/Tests"
  else
    log "swift format host/macos/Sources host/macos/Tests"
    swift format format --in-place --recursive "$ROOT/host/macos/Sources" "$ROOT/host/macos/Tests"
  fi
else
  log "swift format unavailable; skipping Swift formatting"
fi
