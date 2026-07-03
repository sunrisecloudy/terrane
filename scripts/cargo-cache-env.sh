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
# Existing CARGO_TARGET_DIR, CARGO_INCREMENTAL, RUSTC_WRAPPER, SCCACHE_DIR, and
# SCCACHE_CACHE_SIZE values are preserved.
#
# Caching model: share the *content-addressed* layer, isolate the *mutable*
# layer. A cargo target dir is a single-source-tree workspace — many worktrees
# writing one shared dir clobber each other's fingerprints (linking the wrong
# rlib, e.g. a phantom "missing field") and serialize behind cargo's per-dir
# lock. So each worktree gets its OWN target dir, while sccache (below) still
# shares every compiled crate across worktrees and branches by content hash —
# so isolation costs disk, not rebuild time.

TERRANE_CARGO_CACHE_SOURCED=0
case "${ZSH_EVAL_CONTEXT:-}" in
  *:file*) TERRANE_CARGO_CACHE_SOURCED=1 ;;
esac
if [ -n "${BASH_VERSION:-}" ] && [ "${BASH_SOURCE[0]}" != "$0" ]; then
  TERRANE_CARGO_CACHE_SOURCED=1
fi

if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  # Key the target dir by the worktree root so each worktree is isolated.
  # basenames collide (every git worktree is named "terrane"), so hash the
  # absolute path; keep a readable basename prefix for `du`/cleanup.
  terrane_wt_root="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || printf '%s' "$PWD")"
  terrane_wt_hash="$(printf '%s' "$terrane_wt_root" | shasum -a 256 2>/dev/null | cut -c1-16)"
  if [ -z "$terrane_wt_hash" ]; then terrane_wt_hash="default"; fi
  export CARGO_TARGET_DIR="$HOME/Library/Caches/terrane/cargo-target/$(basename "$terrane_wt_root")-$terrane_wt_hash"
  unset terrane_wt_root terrane_wt_hash
fi

# sccache caches a rustc invocation keyed by the hash of its inputs; incremental
# compilation is not cacheable and drags the Rust hit rate down, so turn it off
# whenever sccache is in play. (Per-crate recompiles stay cheap in this
# many-small-crates workspace, and unchanged crates come straight from sccache.)
if [ -z "${CARGO_INCREMENTAL:-}" ]; then
  export CARGO_INCREMENTAL=0
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
  printf 'CARGO_INCREMENTAL=%q\n' "${CARGO_INCREMENTAL:-}"
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
