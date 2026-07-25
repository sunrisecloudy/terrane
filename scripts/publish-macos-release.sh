#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=""
BUILD_NUMBER="1"
OUTPUT="$ROOT/artifacts/macos"

usage() {
  cat <<'EOF'
Usage: scripts/publish-macos-release.sh --version X.Y.Z [options]

Build, sign, notarize, verify, and publish an Apple-silicon Terrane release
from this Mac.

Options:
  --build-number N  CFBundleVersion (default: 1)
  --output DIR      Artifact directory (default: artifacts/macos)
  --help            Show this help

Required environment:
  MACOS_SIGNING_IDENTITY
  TERRANE_CAP_SIGNING_KEY_HEX

Notarization additionally requires either:
  APPLE_NOTARY_KEYCHAIN_PROFILE
or:
  APPLE_NOTARY_KEY_PATH
  APPLE_NOTARY_KEY_ID
  APPLE_NOTARY_ISSUER_ID
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
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
  printf 'release version must be semantic, for example 0.2.0\n' >&2
  exit 2
fi
if [[ ! "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  printf 'build number must be a positive integer\n' >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  printf 'Terrane releases must be published from an Apple-silicon Mac\n' >&2
  exit 1
fi

for command in cargo gh git jq shasum xcodebuild xcodegen; do
  command -v "$command" >/dev/null || {
    printf 'required command is missing: %s\n' "$command" >&2
    exit 1
  }
done

: "${MACOS_SIGNING_IDENTITY:?set MACOS_SIGNING_IDENTITY to a Developer ID Application identity}"
: "${TERRANE_CAP_SIGNING_KEY_HEX:?set TERRANE_CAP_SIGNING_KEY_HEX to the production Ed25519 seed}"

TAG="v$VERSION"
NOTES="$ROOT/docs/releases/$TAG.md"
DMG="$OUTPUT/Terrane-${VERSION}-macos-arm64.dmg"
CHECKSUMS="$OUTPUT/SHA256SUMS"
MANIFEST="$OUTPUT/release-manifest.json"

cd "$ROOT"
if [[ -n "$(git status --porcelain)" ]]; then
  printf 'release publishing requires a clean Git worktree\n' >&2
  exit 1
fi
if [[ ! -f "$NOTES" ]]; then
  printf 'release notes are missing: %s\n' "$NOTES" >&2
  exit 1
fi

git fetch --no-tags origin main
if ! git merge-base --is-ancestor HEAD origin/main; then
  printf 'release commit must be contained in origin/main\n' >&2
  exit 1
fi
if [[ "$(git cat-file -t "refs/tags/$TAG" 2>/dev/null || true)" != "tag" ]]; then
  printf 'create an annotated %s tag at the release commit before publishing\n' "$TAG" >&2
  exit 1
fi
if [[ "$(git rev-list -n 1 "$TAG")" != "$(git rev-parse HEAD)" ]]; then
  printf '%s does not point at the current release commit\n' "$TAG" >&2
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
  mktemp -d "${HOME}/Library/Caches/terrane-release-source.XXXXXX"
)"
DERIVED_DATA="$(mktemp -d "${TMPDIR:-/tmp}/terrane-release-tests.XXXXXX")"
DOWNLOAD="$(mktemp -d "${TMPDIR:-/tmp}/terrane-release-download.XXXXXX")"
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
  printf 'release verification changed tracked source files\n' >&2
  exit 1
fi

scripts/build-macos-release.sh \
  --version "$VERSION" \
  --build-number "$BUILD_NUMBER" \
  --output "$OUTPUT" \
  --notarize

scripts/verify-macos-release.sh "$DMG" "$VERSION"
(
  cd "$OUTPUT"
  shasum -a 256 -c "$(basename "$CHECKSUMS")"
)
jq -e \
  --arg version "$VERSION" \
  --arg commit "$(git rev-parse HEAD)" \
  '
    .version == $version and
    .sourceCommit == $commit and
    .developerIdSigned == true and
    .notarized == true and
    .architecture == "arm64" and
    .minimumSystemVersion == "13.0" and
    .builtInAppBundleCount == 14 and
    .capabilityBundleCount == 42
  ' "$MANIFEST" >/dev/null

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
  --title "Terrane $TAG" \
  --notes-file "$NOTES" \
  "$DMG" \
  "$CHECKSUMS" \
  "$MANIFEST"

gh release download "$TAG" \
  --dir "$DOWNLOAD" \
  --pattern "Terrane-${VERSION}-macos-arm64.dmg" \
  --pattern "SHA256SUMS" \
  --pattern "release-manifest.json"
(
  cd "$DOWNLOAD"
  shasum -a 256 -c SHA256SUMS
)
scripts/verify-macos-release.sh \
  "$DOWNLOAD/Terrane-${VERSION}-macos-arm64.dmg" \
  "$VERSION"
cmp "$MANIFEST" "$DOWNLOAD/release-manifest.json"

printf 'published and re-verified Terrane %s from this Mac\n' "$TAG"
