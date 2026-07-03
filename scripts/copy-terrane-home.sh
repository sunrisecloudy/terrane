#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/copy-terrane-home.sh [--from SOURCE_HOME] [--to TARGET_HOME] [--force]

Copy a Terrane home into the current checkout/worktree.

Defaults:
  --to      "$PWD/.terrane"
  --from    the canonical sibling /Users/vehasuwat/Project/terrane/.terrane
            when present, otherwise $TERRANE_HOME, otherwise ~/.terrane

The copy skips live process state such as locks, sockets, and resident-server
state. It refuses to merge into a non-empty target unless --force is passed.
EOF
}

force=0
source_home="${TERRANE_HOME:-}"
target_home="$PWD/.terrane"
explicit_source=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --from)
      [ "$#" -ge 2 ] || { echo "missing value for --from" >&2; exit 2; }
      source_home="$2"
      explicit_source=1
      shift 2
      ;;
    --to)
      [ "$#" -ge 2 ] || { echo "missing value for --to" >&2; exit 2; }
      target_home="$2"
      shift 2
      ;;
    --force)
      force=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

if [ "$explicit_source" -eq 0 ] && [ -d "/Users/vehasuwat/Project/terrane/.terrane" ]; then
  source_home="/Users/vehasuwat/Project/terrane/.terrane"
elif [ -z "$source_home" ] && [ -d "$HOME/.terrane" ]; then
  source_home="$HOME/.terrane"
elif [ -z "$source_home" ]; then
  echo "no source Terrane home found; pass --from /path/to/.terrane" >&2
  exit 1
fi

source_home="$(cd "$source_home" && pwd)"
target_parent="$(dirname "$target_home")"
target_name="$(basename "$target_home")"
mkdir -p "$target_parent"
target_parent="$(cd "$target_parent" && pwd)"
target_home="$target_parent/$target_name"

if [ ! -d "$source_home" ]; then
  echo "source Terrane home does not exist: $source_home" >&2
  exit 1
fi

if [ "$source_home" = "$target_home" ]; then
  echo "source and target are the same: $source_home" >&2
  exit 1
fi

if [ -d "$target_home" ] && [ -n "$(find "$target_home" -mindepth 1 -maxdepth 1 -print -quit)" ] && [ "$force" -ne 1 ]; then
  echo "target Terrane home is not empty: $target_home" >&2
  echo "pass --force to merge/overwrite files there" >&2
  exit 1
fi

mkdir -p "$target_home"

rsync -a \
  --exclude='*.lock' \
  --exclude='*.sock' \
  --exclude='*.pid' \
  --exclude='engines/mlx-server.json' \
  --exclude='.mcp-drafts/' \
  "$source_home/" "$target_home/"

cat <<EOF
Copied Terrane home:
  from: $source_home
    to: $target_home

Use it with:
  export TERRANE_HOME="$target_home"
EOF

if [ "$target_parent" = "$repo_root" ] && [ "$target_name" = ".terrane" ]; then
  echo
  echo "Note: .terrane/ is gitignored in this repo."
fi
