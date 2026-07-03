#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<'USAGE'
usage: scripts/test.sh [nextest args...]

Runs the workspace tests with the Terrane cargo cache (per-worktree target
dir + sccache) sourced, using cargo-nextest for faster, better-isolated runs.

  (no args)   cargo nextest run --workspace   +   cargo test --workspace --doc
  <args...>   cargo nextest run <args...>     (targeted; doctests skipped)

nextest runs integration/unit tests in parallel processes; it does not run
doctests, so a full run adds a `cargo test --doc` pass. Falls back to
`cargo test` when cargo-nextest is not installed.
USAGE
  exit 0
fi

log() {
  printf '\n==> %s\n' "$*"
}

# Per-worktree CARGO_TARGET_DIR + sccache (same env the agent hook injects).
# shellcheck source=scripts/cargo-cache-env.sh
. "$ROOT/scripts/cargo-cache-env.sh" --quiet

cd "$ROOT"

if ! command -v cargo-nextest >/dev/null 2>&1; then
  log "cargo-nextest not found; falling back to cargo test"
  exec cargo test --workspace "$@"
fi

# --no-tests=warn: many Terrane crates keep their tests in a sibling tests/ dir
# (or in terrane-core/tests/cap/), so a crate-scoped run can legitimately find
# no tests in that crate — warn instead of failing the run.
if [[ "$#" -gt 0 ]]; then
  log "cargo nextest run $*"
  cargo nextest run --no-tests=warn "$@"
else
  log "cargo nextest run --workspace"
  cargo nextest run --no-tests=warn --workspace
  log "cargo test --workspace --doc"
  cargo test --workspace --doc
fi
