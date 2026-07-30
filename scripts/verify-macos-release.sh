#!/usr/bin/env bash
set -euo pipefail

UNSIGNED=0
EXTERNAL_CAPABILITIES=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --unsigned)
      UNSIGNED=1
      shift
      ;;
    --external-capabilities)
      EXTERNAL_CAPABILITIES="${2:-}"
      shift 2
      ;;
    *)
      break
      ;;
  esac
done

DMG="${1:-}"
EXPECTED_VERSION="${2:-}"
if [[ -z "$DMG" || ! -f "$DMG" ]]; then
  printf 'usage: scripts/verify-macos-release.sh [--unsigned] [--external-capabilities DIR] <Terrane.dmg> [expected-version]\n' >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
  printf 'macOS release verification must run on macOS\n' >&2
  exit 1
fi

if [[ "$UNSIGNED" -eq 0 ]]; then
  codesign --verify --strict --verbose=4 "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG"
fi

MOUNT="$(mktemp -d "${TMPDIR:-/tmp}/terrane-release-mount.XXXXXX")"
EXTRACTED="$(mktemp -d "${TMPDIR:-/tmp}/terrane-release-workers.XXXXXX")"
DEVICE=""
cleanup() {
  if [[ -n "$DEVICE" ]]; then
    hdiutil detach "$DEVICE" >/dev/null 2>&1 || true
  fi
  rm -rf "$MOUNT" "$EXTRACTED"
}
trap cleanup EXIT

attach_output="$(hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT" "$DMG")"
DEVICE="$(printf '%s\n' "$attach_output" | awk '/^\/dev\// {print $1; exit}')"
if [[ -z "$DEVICE" ]]; then
  printf 'could not identify the attached DMG device\n' >&2
  exit 1
fi
APP="$MOUNT/Terrane.app"
if [[ ! -d "$APP" ]]; then
  printf 'DMG does not contain Terrane.app\n' >&2
  exit 1
fi

if [[ "$UNSIGNED" -eq 0 ]]; then
  codesign --verify --deep --strict --verbose=4 "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute --verbose=4 "$APP"

  signature="$(codesign -dvv "$APP" 2>&1)"
  if ! grep -q 'Authority=Developer ID Application:' <<<"$signature"; then
    printf 'Terrane.app is not signed with Developer ID Application\n' >&2
    exit 1
  fi
  if grep -q 'TeamIdentifier=not set' <<<"$signature"; then
    printf 'Terrane.app does not have an Apple Developer Team ID\n' >&2
    exit 1
  fi
  if ! grep -q 'flags=.*runtime' <<<"$signature"; then
    printf 'Terrane.app does not have hardened runtime enabled\n' >&2
    exit 1
  fi
fi

EXECUTABLE="$APP/Contents/MacOS/Terrane"
architectures="$(lipo -archs "$EXECUTABLE")"
if [[ "$architectures" != "arm64" ]]; then
  printf 'expected an arm64-only executable, found: %s\n' "$architectures" >&2
  exit 1
fi
minimum_version="$(plutil -extract LSMinimumSystemVersion raw "$APP/Contents/Info.plist")"
if [[ "$minimum_version" != "13.0" ]]; then
  printf 'expected macOS minimum version 13.0, found: %s\n' "$minimum_version" >&2
  exit 1
fi
if [[ -n "$EXPECTED_VERSION" ]]; then
  actual_version="$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")"
  if [[ "$actual_version" != "$EXPECTED_VERSION" ]]; then
    printf 'expected version %s, found %s\n' "$EXPECTED_VERSION" "$actual_version" >&2
    exit 1
  fi
fi

CAPABILITIES="$APP/Contents/Resources/capabilities"
capability_count="$(find "$CAPABILITIES" -maxdepth 1 -type f -name '*.tcap' | wc -l | tr -d ' ')"
ARCHIVE_SOURCE="$CAPABILITIES"
expected_embedded_count=42
if [[ -n "$EXTERNAL_CAPABILITIES" ]]; then
  command -v jq >/dev/null || {
    printf 'jq is required to verify external capabilities\n' >&2
    exit 1
  }
  if [[ ! -d "$EXTERNAL_CAPABILITIES" ]]; then
    printf 'external capability directory does not exist: %s\n' \
      "$EXTERNAL_CAPABILITIES" >&2
    exit 1
  fi
  expected_embedded_count=0
  ARCHIVE_SOURCE="$EXTERNAL_CAPABILITIES"
  index_count="$(jq -r '.artifacts | length' "$CAPABILITIES/index.json")"
  external_count="$(find "$ARCHIVE_SOURCE" -maxdepth 1 -type f -name '*.tcap' | wc -l | tr -d ' ')"
  download_base_url="$(jq -r '.downloadBaseUrl // empty' "$CAPABILITIES/index.json")"
  if [[ "$index_count" != "42" || "$external_count" != "42" ||
        -z "$download_base_url" ]]; then
    printf 'invalid external capability set: index=%s archives=%s download=%s\n' \
      "$index_count" "$external_count" "$download_base_url" >&2
    exit 1
  fi
fi
if [[ "$capability_count" != "$expected_embedded_count" ]]; then
  printf 'expected %s embedded capability archives, found %s\n' \
    "$expected_embedded_count" "$capability_count" >&2
  exit 1
fi

app_bundle_count="$(find "$APP/Contents/Resources/apps" -mindepth 2 -maxdepth 2 -type f -name manifest.json | wc -l | tr -d ' ')"
if [[ "$app_bundle_count" != "17" ]]; then
  printf 'expected 17 built-in app bundles, found %s\n' "$app_bundle_count" >&2
  exit 1
fi
for archive in "$ARCHIVE_SOURCE"/*.tcap; do
  namespace="$(basename "$archive" .tcap)"
  if [[ -n "$EXTERNAL_CAPABILITIES" ]]; then
    expected_archive_sha="$(
      jq -r \
        --arg archive "$(basename "$archive")" \
        '.artifacts[] | select(.archive == $archive) | .archiveSha256' \
        "$CAPABILITIES/index.json"
    )"
    actual_archive_sha="$(shasum -a 256 "$archive" | awk '{print $1}')"
    if [[ -z "$expected_archive_sha" ||
          "$actual_archive_sha" != "$expected_archive_sha" ]]; then
      printf 'external capability hash mismatch: %s\n' "$archive" >&2
      exit 1
    fi
  fi
  destination="$EXTRACTED/$namespace"
  mkdir -p "$destination"
  tar -xf "$archive" -C "$destination"
  worker="$(find "$destination" -maxdepth 1 -type f -name 'terrane-cap-*-worker' -print -quit)"
  if [[ -z "$worker" ]]; then
    printf 'capability archive has no worker executable: %s\n' "$archive" >&2
    exit 1
  fi
  if [[ "$UNSIGNED" -eq 0 ]]; then
    codesign --verify --strict --verbose=2 "$worker"
  fi
  if [[ "$(lipo -archs "$worker")" != "arm64" ]]; then
    printf 'capability worker is not arm64-only: %s\n' "$worker" >&2
    exit 1
  fi
done

if [[ ! -f "$APP/Contents/Resources/Assets.car" ]]; then
  printf 'compiled application icon catalog is missing\n' >&2
  exit 1
fi

if [[ "$UNSIGNED" -eq 1 ]]; then
  printf 'verified unsigned Terrane preview contents: %s\n' "$DMG"
else
  printf 'verified production Terrane release: %s\n' "$DMG"
fi
