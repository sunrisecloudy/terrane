#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=scripts/cargo-cache-env.sh
. "$ROOT/scripts/cargo-cache-env.sh" --quiet

if [[ "$#" -eq 0 || "${1:-}" == "--print" ]]; then
  printf 'CARGO_TARGET_DIR=%q\n' "$CARGO_TARGET_DIR"
  printf 'SCCACHE_DIR=%q\n' "$SCCACHE_DIR"
  printf 'SCCACHE_CACHE_SIZE=%q\n' "$SCCACHE_CACHE_SIZE"
  printf 'RUSTC_WRAPPER=%q\n' "${RUSTC_WRAPPER:-}"
  exit 0
fi

exec "$@"
