#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=""
PREVIEW_NUMBER=""
BUILD_NUMBER="1"
OUTPUT="$ROOT/artifacts/macos-preview"
PREVIEW_CAP_SIGNING_KEY_HEX="000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"

usage() {
  cat <<'EOF'
Usage: scripts/publish-macos-preview.sh --version X.Y.Z --preview N [options]

Build, verify, and publish an explicitly unsigned Apple-silicon preview from
this Mac. This is not a production or Gatekeeper-trusted release.

Options:
  --build-number N  CFBundleVersion (default: 1)
  --output DIR      Artifact directory (default: artifacts/macos-preview)
  --help            Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --preview)
      PREVIEW_NUMBER="${2:-}"
      shift 2
      ;;
    --build-number)
      BUILD_NUMBER="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'preview version must be semantic, for example 0.2.0\n' >&2
  exit 2
fi
if [[ ! "$PREVIEW_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  printf 'preview number must be a positive integer\n' >&2
  exit 2
fi
if [[ ! "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  printf 'build number must be a positive integer\n' >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  printf 'Terrane macOS previews must be published from an Apple-silicon Mac\n' >&2
  exit 1
fi

for command in cargo cmp gh git jq shasum xcodebuild xcodegen; do
  command -v "$command" >/dev/null || {
    printf 'required command is missing: %s\n' "$command" >&2
    exit 1
  }
done

TAG="v${VERSION}-preview.${PREVIEW_NUMBER}"
NOTES="$ROOT/docs/releases/$TAG.md"
DMG="$OUTPUT/Terrane-${VERSION}-macos-arm64.dmg"
CHECKSUMS="$OUTPUT/SHA256SUMS"
MANIFEST="$OUTPUT/release-manifest.json"
CAPABILITY_BASE_URL="https://github.com/sunrisecloudy/terrane/releases/download/$TAG"
MAX_DMG_BYTES=41943040

cd "$ROOT"
if [[ -n "$(git status --porcelain)" ]]; then
  printf 'preview publishing requires a clean Git worktree\n' >&2
  exit 1
fi
if [[ ! -f "$NOTES" ]]; then
  printf 'preview release notes are missing: %s\n' "$NOTES" >&2
  exit 1
fi

git fetch --no-tags origin main
if ! git merge-base --is-ancestor HEAD origin/main; then
  printf 'preview commit must be contained in origin/main\n' >&2
  exit 1
fi
if [[ "$(git cat-file -t "refs/tags/$TAG" 2>/dev/null || true)" != "tag" ]]; then
  printf 'create an annotated %s tag at the preview commit before publishing\n' "$TAG" >&2
  exit 1
fi
if [[ "$(git rev-list -n 1 "$TAG")" != "$(git rev-parse HEAD)" ]]; then
  printf '%s does not point at the current preview commit\n' "$TAG" >&2
  exit 1
fi
if gh release view "$TAG" >/dev/null 2>&1; then
  printf 'GitHub release already exists: %s\n' "$TAG" >&2
  exit 1
fi

scripts/with-cargo-cache.sh cargo fmt --all --check
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings

NATIVE_TEST_SOURCE="$(
  mktemp -d "${HOME}/Library/Caches/terrane-preview-source.XXXXXX"
)"
DERIVED_DATA="$(mktemp -d "${TMPDIR:-/tmp}/terrane-preview-tests.XXXXXX")"
DOWNLOAD="$(mktemp -d "${TMPDIR:-/tmp}/terrane-preview-download.XXXXXX")"
cleanup() {
  rm -rf "$NATIVE_TEST_SOURCE" "$DERIVED_DATA" "$DOWNLOAD"
}
trap cleanup EXIT

git archive HEAD | tar -x -C "$NATIVE_TEST_SOURCE"
cd "$NATIVE_TEST_SOURCE"
xcodegen generate --spec host/macos/project.yml
xcodebuild \
  -quiet \
  -project host/macos/Terrane.xcodeproj \
  -scheme TerraneHostE2ETests \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$DERIVED_DATA" \
  CODE_SIGNING_ALLOWED=NO \
  test
cd "$ROOT"

if [[ -n "$(git status --porcelain)" ]]; then
  printf 'preview verification changed tracked source files\n' >&2
  exit 1
fi

TERRANE_CAP_SIGNING_KEY_HEX="$PREVIEW_CAP_SIGNING_KEY_HEX" \
TERRANE_CAP_INDEX_BASE_URL="$CAPABILITY_BASE_URL" \
  scripts/build-macos-release.sh \
    --version "$VERSION" \
    --build-number "$BUILD_NUMBER" \
    --output "$OUTPUT" \
    --unsigned \
    --external-capabilities \
    --max-dmg-bytes "$MAX_DMG_BYTES"

scripts/verify-macos-release.sh \
  --unsigned \
  --external-capabilities "$OUTPUT" \
  "$DMG" \
  "$VERSION"
(
  cd "$OUTPUT"
  shasum -a 256 -c "$(basename "$CHECKSUMS")"
)
jq -e \
  --arg version "$VERSION" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg preview_key \
    "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8" \
  '
    .version == $version and
    .sourceCommit == $commit and
    .developerIdSigned == false and
    .notarized == false and
    .architecture == "arm64" and
    .minimumSystemVersion == "13.0" and
    .builtInAppBundleCount == 14 and
    .capabilityBundleCount == 42 and
    .embeddedCapabilityBundleCount == 0 and
    .capabilityDelivery == "on-demand" and
    .capabilityVerifyingKey == $preview_key
  ' "$MANIFEST" >/dev/null

CAPABILITY_ASSETS=("$OUTPUT"/*.tcap)
if [[ "${#CAPABILITY_ASSETS[@]}" -ne 42 ]]; then
  printf 'expected 42 external capability assets, found %s\n' \
    "${#CAPABILITY_ASSETS[@]}" >&2
  exit 1
fi

remote_tag_commit="$(
  git ls-remote --tags origin "refs/tags/$TAG^{}" |
    awk 'NR == 1 {print $1}'
)"
if [[ -n "$remote_tag_commit" && "$remote_tag_commit" != "$(git rev-parse HEAD)" ]]; then
  printf 'remote tag %s points to a different commit\n' "$TAG" >&2
  exit 1
fi
if [[ -z "$remote_tag_commit" ]]; then
  git push origin "$TAG"
fi

gh release create "$TAG" \
  --verify-tag \
  --prerelease \
  --title "Terrane ${VERSION} Preview ${PREVIEW_NUMBER} (Unsigned)" \
  --notes-file "$NOTES" \
  "$DMG" \
  "$CHECKSUMS" \
  "$MANIFEST" \
  "${CAPABILITY_ASSETS[@]}"

gh release download "$TAG" \
  --dir "$DOWNLOAD" \
  --pattern "Terrane-${VERSION}-macos-arm64.dmg" \
  --pattern "SHA256SUMS" \
  --pattern "release-manifest.json" \
  --pattern "*.tcap"
(
  cd "$DOWNLOAD"
  shasum -a 256 -c SHA256SUMS
)
scripts/verify-macos-release.sh \
  --unsigned \
  --external-capabilities "$DOWNLOAD" \
  "$DOWNLOAD/Terrane-${VERSION}-macos-arm64.dmg" \
  "$VERSION"
cmp "$MANIFEST" "$DOWNLOAD/release-manifest.json"

printf 'published and re-verified unsigned Terrane preview %s\n' "$TAG"
