#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

log() {
  printf '\n==> %s\n' "$*"
}

run_in() {
  local dir="$1"
  shift
  log "(cd ${dir#$ROOT/} && $*)"
  (cd "$dir" && "$@")
}

rust_workspaces=(
  "$ROOT/rust"
  "$ROOT/host/cli"
  "$ROOT/host/mcp"
  "$ROOT/host/web"
)

log "scripts/format.sh --check"
"$ROOT/scripts/format.sh" --check

for workspace in "${rust_workspaces[@]}"; do
  run_in "$workspace" cargo clippy --all-targets --locked -- -D warnings
done

js_roots=(
  "$ROOT/apps"
  "$ROOT/host"
  "$ROOT/tools"
)

js_files=()
while IFS= read -r file; do
  js_files+=("$file")
done < <(
  find "${js_roots[@]}" \
    -path '*/target/*' -prune -o \
    -path '*/dist/*' -prune -o \
    -path '*/vendor/*' -prune -o \
    -path '*/.derived/*' -prune -o \
    -path '*/build/*' -prune -o \
    -path '*/node_modules/*' -prune -o \
    \( -name '*.js' -o -name '*.mjs' \) -print \
    | sort
)

for file in "${js_files[@]}"; do
  log "node --check ${file#$ROOT/}"
  node --check "$file"
done

react_app_manifests=()
while IFS= read -r manifest; do
  react_app_manifests+=("$manifest")
done < <(
  find "$ROOT/apps" -name manifest.json -print \
    | while IFS= read -r manifest; do
        if grep -q '"frontend"' "$manifest"; then
          printf '%s\n' "$manifest"
        fi
      done \
    | sort
)

for manifest in "${react_app_manifests[@]}"; do
  app_dir="$(dirname "$manifest")"
  rel_app_dir="../${app_dir#$ROOT/}"
  run_in "$ROOT/rust" cargo run -p terrane-app-build -- --check "$rel_app_dir"
done
