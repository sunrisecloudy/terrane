#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=""
BUILD_NUMBER="1"
OUTPUT="$ROOT/artifacts/macos"
UNSIGNED=0
NOTARIZE=0
EXTERNAL_CAPABILITIES=0
MAX_DMG_BYTES=""

usage() {
  cat <<'EOF'
Usage: scripts/build-macos-release.sh --version X.Y.Z [options]

Build an Apple-silicon-only Terrane DMG.

Options:
  --build-number N  CFBundleVersion (default: 1)
  --output DIR      Final artifact directory (default: artifacts/macos)
  --unsigned        Build a local validation artifact without Developer ID
  --notarize        Notarize and staple both the app and DMG
  --external-capabilities
                    Embed only the signed capability index; emit the 42
                    archives beside the DMG for on-demand download
  --max-dmg-bytes N Fail if the final DMG exceeds N bytes
  --help            Show this help

Signed builds require:
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
    --unsigned)
      UNSIGNED=1
      shift
      ;;
    --notarize)
      NOTARIZE=1
      shift
      ;;
    --external-capabilities)
      EXTERNAL_CAPABILITIES=1
      shift
      ;;
    --max-dmg-bytes)
      MAX_DMG_BYTES="${2:-}"
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

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  printf 'release version must be semantic, for example 0.2.0\n' >&2
  exit 2
fi
if [[ ! "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  printf 'build number must be a positive integer\n' >&2
  exit 2
fi
if [[ -n "$MAX_DMG_BYTES" && ! "$MAX_DMG_BYTES" =~ ^[1-9][0-9]*$ ]]; then
  printf 'maximum DMG bytes must be a positive integer\n' >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  printf 'Terrane macOS releases must be built on an Apple-silicon Mac\n' >&2
  exit 1
fi

for command in cargo codesign ditto hdiutil plutil shasum xcodebuild xcodegen; do
  command -v "$command" >/dev/null || {
    printf 'required command is missing: %s\n' "$command" >&2
    exit 1
  }
done

: "${TERRANE_CAP_SIGNING_KEY_HEX:?set TERRANE_CAP_SIGNING_KEY_HEX to the production Ed25519 seed}"
if [[ "$EXTERNAL_CAPABILITIES" -eq 1 ]]; then
  : "${TERRANE_CAP_INDEX_BASE_URL:?set TERRANE_CAP_INDEX_BASE_URL for external capability downloads}"
fi
if [[ "$UNSIGNED" -eq 0 ]]; then
  : "${MACOS_SIGNING_IDENTITY:?set MACOS_SIGNING_IDENTITY to a Developer ID Application identity}"
  if [[ -n "$(git -C "$ROOT" status --porcelain)" ]]; then
    printf 'signed releases require a clean Git worktree\n' >&2
    exit 1
  fi
  if ! security find-identity -v -p codesigning | grep -F "$MACOS_SIGNING_IDENTITY" >/dev/null; then
    printf 'signing identity is not available: %s\n' "$MACOS_SIGNING_IDENTITY" >&2
    exit 1
  fi
fi
if [[ "$NOTARIZE" -eq 1 && "$UNSIGNED" -eq 1 ]]; then
  printf -- '--notarize cannot be combined with --unsigned\n' >&2
  exit 2
fi

notary_args=()
if [[ "$NOTARIZE" -eq 1 ]]; then
  if [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    notary_args=(--keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE")
  else
    : "${APPLE_NOTARY_KEY_PATH:?set APPLE_NOTARY_KEY_PATH or APPLE_NOTARY_KEYCHAIN_PROFILE}"
    : "${APPLE_NOTARY_KEY_ID:?set APPLE_NOTARY_KEY_ID}"
    : "${APPLE_NOTARY_ISSUER_ID:?set APPLE_NOTARY_ISSUER_ID}"
    notary_args=(
      --key "$APPLE_NOTARY_KEY_PATH"
      --key-id "$APPLE_NOTARY_KEY_ID"
      --issuer "$APPLE_NOTARY_ISSUER_ID"
    )
  fi
fi

mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"
WORK="$(mktemp -d "$OUTPUT/.terrane-release.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
CAPABILITIES="$WORK/capabilities"
EMBEDDED_CAPABILITIES="$WORK/embedded-capabilities"
DERIVED_DATA="$WORK/DerivedData"
BUILD_DIR="$WORK/build"
STAGING="$WORK/dmg"
APP="$BUILD_DIR/Terrane.app"
DMG="$OUTPUT/Terrane-${VERSION}-macos-arm64.dmg"
MANIFEST="$OUTPUT/release-manifest.json"
CHECKSUMS="$OUTPUT/SHA256SUMS"

for final_path in "$DMG" "$MANIFEST" "$CHECKSUMS"; do
  if [[ -e "$final_path" ]]; then
    printf 'refusing to overwrite existing release artifact: %s\n' "$final_path" >&2
    exit 1
  fi
done
if [[ "$EXTERNAL_CAPABILITIES" -eq 1 ]] &&
   compgen -G "$OUTPUT/*.tcap" >/dev/null; then
  printf 'refusing to overwrite existing capability artifacts in %s\n' "$OUTPUT" >&2
  exit 1
fi

cd "$ROOT"
xcodegen generate --spec host/macos/project.yml

if [[ "$UNSIGNED" -eq 0 ]]; then
  TERRANE_MACOS_CODESIGN_IDENTITY="$MACOS_SIGNING_IDENTITY" \
    scripts/package-native-capabilities.sh "$CAPABILITIES"
else
  scripts/package-native-capabilities.sh "$CAPABILITIES"
fi

CAPABILITY_BUILD_SOURCE="$CAPABILITIES"
if [[ "$EXTERNAL_CAPABILITIES" -eq 1 ]]; then
  mkdir -p "$EMBEDDED_CAPABILITIES"
  cp "$CAPABILITIES/index.json" "$CAPABILITIES/verifying-key.hex" \
    "$EMBEDDED_CAPABILITIES/"
  CAPABILITY_BUILD_SOURCE="$EMBEDDED_CAPABILITIES"
fi

TERRANE_CAP_BUNDLE_DIR="$CAPABILITY_BUILD_SOURCE" \
TERRANE_CAP_EXTERNAL="$EXTERNAL_CAPABILITIES" \
xcodebuild \
  -quiet \
  -project host/macos/Terrane.xcodeproj \
  -scheme TerraneHost \
  -configuration DeveloperID \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$DERIVED_DATA" \
  CONFIGURATION_BUILD_DIR="$BUILD_DIR" \
  CODE_SIGNING_ALLOWED=NO \
  MARKETING_VERSION="$VERSION" \
  CURRENT_PROJECT_VERSION="$BUILD_NUMBER" \
  build

if [[ ! -d "$APP" ]]; then
  printf 'Release build did not produce %s\n' "$APP" >&2
  exit 1
fi

/usr/bin/strip -S -x "$APP/Contents/MacOS/Terrane"

actual_version="$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")"
actual_build="$(plutil -extract CFBundleVersion raw "$APP/Contents/Info.plist")"
if [[ "$actual_version" != "$VERSION" || "$actual_build" != "$BUILD_NUMBER" ]]; then
  printf 'bundle version mismatch: expected %s (%s), found %s (%s)\n' \
    "$VERSION" "$BUILD_NUMBER" "$actual_version" "$actual_build" >&2
  exit 1
fi

capability_count="$(find "$APP/Contents/Resources/capabilities" -maxdepth 1 -type f -name '*.tcap' | wc -l | tr -d ' ')"
expected_embedded_capabilities=42
if [[ "$EXTERNAL_CAPABILITIES" -eq 1 ]]; then
  expected_embedded_capabilities=0
fi
if [[ "$capability_count" != "$expected_embedded_capabilities" ]]; then
  printf 'expected %s embedded capabilities, found %s\n' \
    "$expected_embedded_capabilities" "$capability_count" >&2
  exit 1
fi
if [[ ! -f "$APP/Contents/Resources/capabilities/index.json" ||
      ! -f "$APP/Contents/Resources/capabilities/verifying-key.hex" ]]; then
  printf 'capability index or verifying key is missing from the app\n' >&2
  exit 1
fi

app_bundle_count="$(find "$APP/Contents/Resources/apps" -mindepth 2 -maxdepth 2 -type f -name manifest.json | wc -l | tr -d ' ')"
if [[ "$app_bundle_count" != "14" ]]; then
  printf 'expected 14 built-in app bundles, found %s\n' "$app_bundle_count" >&2
  exit 1
fi

if [[ "$UNSIGNED" -eq 0 ]]; then
  codesign --force --timestamp --options runtime \
    --entitlements host/macos/Sources/TerraneHostDeveloperID.entitlements \
    --sign "$MACOS_SIGNING_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=4 "$APP"
  if ! codesign -dvv "$APP" 2>&1 | grep -q 'flags=.*runtime'; then
    printf 'signed app does not have hardened runtime enabled\n' >&2
    exit 1
  fi
fi

if [[ "$NOTARIZE" -eq 1 ]]; then
  APP_ZIP="$WORK/Terrane-${VERSION}-macos-arm64.zip"
  ditto -c -k --keepParent "$APP" "$APP_ZIP"
  xcrun notarytool submit "$APP_ZIP" "${notary_args[@]}" --wait
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute --verbose=4 "$APP"
fi

mkdir -p "$STAGING"
ditto "$APP" "$STAGING/Terrane.app"
ln -s /Applications "$STAGING/Applications"
hdiutil create \
  -volname "Terrane ${VERSION}" \
  -srcfolder "$STAGING" \
  -ov \
  -format UDZO \
  "$DMG"

if [[ "$UNSIGNED" -eq 0 ]]; then
  codesign --force --timestamp --sign "$MACOS_SIGNING_IDENTITY" "$DMG"
  codesign --verify --strict --verbose=4 "$DMG"
fi

if [[ "$NOTARIZE" -eq 1 ]]; then
  xcrun notarytool submit "$DMG" "${notary_args[@]}" --wait
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG"
fi

dmg_sha="$(shasum -a 256 "$DMG" | awk '{print $1}')"
dmg_bytes="$(stat -f '%z' "$DMG")"
if [[ -n "$MAX_DMG_BYTES" && "$dmg_bytes" -gt "$MAX_DMG_BYTES" ]]; then
  printf 'DMG size budget exceeded: %s bytes > %s bytes\n' \
    "$dmg_bytes" "$MAX_DMG_BYTES" >&2
  exit 1
fi
capability_key="$(tr -d '\n' < "$CAPABILITIES/verifying-key.hex")"
commit="$(git rev-parse HEAD)"
build_date="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
signed=true
notarized=false
if [[ "$UNSIGNED" -eq 1 ]]; then
  signed=false
fi
if [[ "$NOTARIZE" -eq 1 ]]; then
  notarized=true
fi
capability_delivery=embedded
if [[ "$EXTERNAL_CAPABILITIES" -eq 1 ]]; then
  capability_delivery=on-demand
  cp "$CAPABILITIES"/*.tcap "$OUTPUT/"
fi

printf '%s  %s\n' "$dmg_sha" "$(basename "$DMG")" > "$CHECKSUMS"
cat > "$MANIFEST" <<EOF
{
  "schemaVersion": 1,
  "product": "Terrane",
  "version": "$VERSION",
  "buildNumber": "$BUILD_NUMBER",
  "sourceCommit": "$commit",
  "builtAt": "$build_date",
  "platform": "macOS",
  "architecture": "arm64",
  "minimumSystemVersion": "13.0",
  "developerIdSigned": $signed,
  "notarized": $notarized,
  "builtInAppBundleCount": 14,
  "capabilityBundleCount": 42,
  "embeddedCapabilityBundleCount": $capability_count,
  "capabilityDelivery": "$capability_delivery",
  "capabilityVerifyingKey": "$capability_key",
  "artifacts": [
    {
      "name": "$(basename "$DMG")",
      "bytes": $dmg_bytes,
      "sha256": "$dmg_sha"
    }
  ]
}
EOF

plutil -lint "$APP/Contents/Info.plist" >/dev/null
if command -v jq >/dev/null; then
  jq -e . "$MANIFEST" >/dev/null
fi

printf 'Terrane macOS release artifacts:\n'
printf '  %s\n  %s\n  %s\n' "$DMG" "$MANIFEST" "$CHECKSUMS"
