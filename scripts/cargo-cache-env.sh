#!/usr/bin/env bash
# Source this file before running Cargo from Terrane worktrees:
#
#   source scripts/cargo-cache-env.sh
#   cargo test --workspace --locked
#
# Or inspect the values without changing the parent shell:
#
#   scripts/cargo-cache-env.sh --print
#
# Existing CARGO_TARGET_DIR, RUSTC_WRAPPER, SCCACHE_DIR, and
# SCCACHE_CACHE_SIZE values are preserved.

TERRANE_CARGO_CACHE_SOURCED=0
case "${ZSH_EVAL_CONTEXT:-}" in
  *:file*) TERRANE_CARGO_CACHE_SOURCED=1 ;;
esac
if [ -n "${BASH_VERSION:-}" ] && [ "${BASH_SOURCE[0]}" != "$0" ]; then
  TERRANE_CARGO_CACHE_SOURCED=1
fi

if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  export CARGO_TARGET_DIR="$HOME/Library/Caches/terrane/cargo-target/all"
fi

if [ -z "${SCCACHE_DIR:-}" ]; then
  export SCCACHE_DIR="$HOME/Library/Caches/sccache"
fi

if [ -z "${SCCACHE_CACHE_SIZE:-}" ]; then
  export SCCACHE_CACHE_SIZE="40G"
fi

if [ -z "${RUSTC_WRAPPER:-}" ]; then
  if command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER="$(command -v sccache)"
  fi
fi

if [ "${1:-}" = "--print" ]; then
  printf 'CARGO_TARGET_DIR=%q\n' "$CARGO_TARGET_DIR"
  printf 'SCCACHE_DIR=%q\n' "$SCCACHE_DIR"
  printf 'SCCACHE_CACHE_SIZE=%q\n' "$SCCACHE_CACHE_SIZE"
  printf 'RUSTC_WRAPPER=%q\n' "${RUSTC_WRAPPER:-}"
elif [ "$TERRANE_CARGO_CACHE_SOURCED" -eq 0 ]; then
  cat <<'USAGE' >&2
Source this script so the exports affect your current shell:

  source scripts/cargo-cache-env.sh

Use --print to inspect the cache environment.
USAGE
fi
