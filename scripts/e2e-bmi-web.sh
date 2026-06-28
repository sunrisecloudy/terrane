#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

log() {
  printf '\n==> %s\n' "$*"
}

log "Validate BMI React app build inputs"
(cd "$ROOT/rust" && cargo run -p terrane-app-build -- --check ../apps/bmi-calculator)

log "Run host/web BMI e2e"
(cd "$ROOT/host/web" && cargo test --test web serves_bmi_calculator_shell_frame_assets_and_backend -- --nocapture)
