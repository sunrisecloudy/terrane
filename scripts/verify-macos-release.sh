#!/usr/bin/env bash
set -euo pipefail

UNSIGNED=0
if [[ "${1:-}" == "--unsigned" ]]; then
  UNSIGNED=1
  shift
fi

DMG="${1:-}"
EXPECTED_VERSION="${2:-}"
if [[ -z "$DMG" || ! -f "$DMG" ]]; then
  printf 'usage: scripts/verify-macos-release.sh [--unsigned] <Terrane.dmg> [expected-version]\n' >&2
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
if [[ "$capability_count" != "42" ]]; then
  printf 'expected 42 capability archives, found %s\n' "$capability_count" >&2
  exit 1
fi

app_bundle_count="$(find "$APP/Contents/Resources/apps" -mindepth 2 -maxdepth 2 -type f -name manifest.json | wc -l | tr -d ' ')"
if [[ "$app_bundle_count" != "14" ]]; then
  printf 'expected 14 built-in app bundles, found %s\n' "$app_bundle_count" >&2
  exit 1
fi
for archive in "$CAPABILITIES"/*.tcap; do
  namespace="$(basename "$archive" .tcap)"
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
