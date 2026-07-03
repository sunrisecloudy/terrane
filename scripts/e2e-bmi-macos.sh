#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

log() {
  printf '\n==> %s\n' "$*"
}

log "Validate BMI React app build inputs"
(cd "$ROOT" && cargo run -p terrane-app-build -- --check apps/bmi-calculator)

log "Generate macOS Xcode project"
(cd "$ROOT/host/macos" && xcodegen generate)

log "Run macOS BMI e2e"
E2E_HOME="${TERRANE_BMI_E2E_HOME:-$(mktemp -d "${TMPDIR:-/tmp}/terrane-bmi-macos.XXXXXX")}"
(
  cd "$ROOT/host/macos"
  export TERRANE_HOME="$E2E_HOME"
  export TERRANE_REPO="$ROOT"
  xcodebuild \
    -project Terrane.xcodeproj \
    -scheme TerraneHost \
    -configuration Debug \
    -destination "platform=macOS" \
    -derivedDataPath ./.derived \
    CONFIGURATION_BUILD_DIR="$PWD/build/Debug" \
    CODE_SIGNING_ALLOWED=NO \
    build

  xcodebuild \
    -project Terrane.xcodeproj \
    -scheme TerraneHostE2ETests \
    -configuration Debug \
    -destination "platform=macOS" \
    -derivedDataPath ./.derived \
    CONFIGURATION_BUILD_DIR="$PWD/build/Debug" \
    CODE_SIGNING_ALLOWED=NO \
    test
)
