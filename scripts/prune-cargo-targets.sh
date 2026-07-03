#!/usr/bin/env bash
set -euo pipefail

# Prune per-worktree cargo target dirs whose git worktree no longer exists.
#
# scripts/cargo-cache-env.sh gives every worktree its own target dir named
# "<basename>-<path-hash>" under the cache root. When a worktree is removed the
# dir is orphaned (~2GB each); this reclaims those. Dry-run by default.

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# MUST match the cache root in scripts/cargo-cache-env.sh.
CACHE_ROOT="$HOME/Library/Caches/terrane/cargo-target"

APPLY=0
INCLUDE_LEGACY=0
for arg in "$@"; do
  case "$arg" in
    -y | --yes) APPLY=1 ;;
    --include-legacy) INCLUDE_LEGACY=1 ;;
    -h | --help)
      cat <<'USAGE'
usage: scripts/prune-cargo-targets.sh [--yes] [--include-legacy]

Lists per-worktree cargo target dirs and marks each LIVE or STALE (its git
worktree is gone). Without --yes this is a dry run.

  --yes             delete the STALE dirs
  --include-legacy  also treat the pre-isolation shared "all" dir as STALE
                    (only safe once every worktree has the per-worktree fix)
USAGE
      exit 0
      ;;
    *)
      printf 'unknown option: %s (see --help)\n' "$arg" >&2
      exit 2
      ;;
  esac
done

# Name a worktree's target dir the same way cargo-cache-env.sh does.
dir_name_for() {
  local path="$1"
  printf '%s-%s' \
    "$(basename "$path")" \
    "$(printf '%s' "$path" | shasum -a 256 | cut -c1-16)"
}

if [[ ! -d "$CACHE_ROOT" ]]; then
  printf 'no cargo cache dir at %s — nothing to prune\n' "$CACHE_ROOT"
  exit 0
fi

# Build the list of live worktree target-dir names (plain array, so this runs
# on macOS's stock bash 3.2 — no associative arrays).
live_names=()
while IFS= read -r line; do
  case "$line" in
    "worktree "*) live_names+=("$(dir_name_for "${line#worktree }")") ;;
  esac
done < <(git -C "$ROOT" worktree list --porcelain)

is_live() {
  local target="$1" name
  for name in "${live_names[@]:-}"; do
    [[ "$name" == "$target" ]] && return 0
  done
  return 1
}

stale=()
for dir in "$CACHE_ROOT"/*/; do
  [[ -d "$dir" ]] || continue
  name="$(basename "$dir")"
  size="$(du -sh "$dir" 2>/dev/null | cut -f1)"
  if is_live "$name"; then
    printf '  LIVE    %6s  %s\n' "$size" "$name"
  elif [[ "$name" == "all" && "$INCLUDE_LEGACY" -eq 0 ]]; then
    printf '  legacy  %6s  %s  (shared; pass --include-legacy to remove)\n' "$size" "$name"
  else
    printf '  STALE   %6s  %s\n' "$size" "$name"
    stale+=("$dir")
  fi
done

if [[ "${#stale[@]}" -eq 0 ]]; then
  printf '\nNothing stale to prune.\n'
  exit 0
fi

if [[ "$APPLY" -eq 1 ]]; then
  printf '\nRemoving %d stale dir(s)...\n' "${#stale[@]}"
  for dir in "${stale[@]}"; do
    rm -rf "$dir"
  done
  printf 'Done.\n'
else
  printf '\n%d stale dir(s) above. Re-run with --yes to delete them.\n' "${#stale[@]}"
fi
