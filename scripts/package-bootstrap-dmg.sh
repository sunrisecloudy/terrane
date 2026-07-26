#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: scripts/package-bootstrap-dmg.sh /path/to/Terrane.app output.dmg" >&2
  exit 2
fi

BOOTSTRAP_APP="$(cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")"
OUTPUT_DMG="$(cd -- "$(dirname -- "$2")" && pwd)/$(basename -- "$2")"
if [[ ! -d "$BOOTSTRAP_APP" ]]; then
  echo "bootstrap app does not exist: $BOOTSTRAP_APP" >&2
  exit 1
fi

/usr/bin/codesign --verify --deep --strict --verbose=2 "$BOOTSTRAP_APP"
ARCHS="$(/usr/bin/lipo -archs "$BOOTSTRAP_APP/Contents/MacOS/TerraneBootstrap")"
if [[ "$ARCHS" != "arm64" ]]; then
  echo "bootstrap must be arm64-only, found: $ARCHS" >&2
  exit 1
fi

STAGING="$(mktemp -d "${TMPDIR:-/tmp}/terrane-bootstrap-dmg.XXXXXX")"
trap 'rm -rf "$STAGING"' EXIT
/usr/bin/ditto "$BOOTSTRAP_APP" "$STAGING/Terrane.app"
ln -s /Applications "$STAGING/Applications"

rm -f "$OUTPUT_DMG"
/usr/bin/hdiutil create \
  -volname "Terrane" \
  -fs HFS+ \
  -format UDBZ \
  -srcfolder "$STAGING" \
  "$OUTPUT_DMG"

/usr/bin/hdiutil verify "$OUTPUT_DMG"
echo "bootstrap_dmg=$OUTPUT_DMG"
stat -f 'size=%z' "$OUTPUT_DMG"
