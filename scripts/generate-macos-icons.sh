#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/host/macos/SharedAssets/TerraneIcon.svg"
OUTPUT_DIR="$ROOT/host/macos/SharedAssets"
OUTPUT_PREVIEW="$OUTPUT_DIR/TerraneIcon-1024.png"
APP_ICON_SET="$OUTPUT_DIR/TerraneAssets.xcassets/AppIcon.appiconset"

if ! command -v rsvg-convert >/dev/null 2>&1; then
  echo "rsvg-convert is required (brew install librsvg)" >&2
  exit 1
fi
if [[ ! -f "$SOURCE" ]]; then
  echo "icon source does not exist: $SOURCE" >&2
  exit 1
fi

mkdir -p "$APP_ICON_SET"

render() {
  local pixels="$1"
  local destination="$2"
  rsvg-convert \
    --width "$pixels" \
    --height "$pixels" \
    --keep-aspect-ratio \
    "$SOURCE" \
    --output "$destination"
}

render 16 "$APP_ICON_SET/icon_16x16.png"
render 32 "$APP_ICON_SET/icon_16x16@2x.png"
render 32 "$APP_ICON_SET/icon_32x32.png"
render 64 "$APP_ICON_SET/icon_32x32@2x.png"
render 128 "$APP_ICON_SET/icon_128x128.png"
render 256 "$APP_ICON_SET/icon_128x128@2x.png"
render 256 "$APP_ICON_SET/icon_256x256.png"
render 512 "$APP_ICON_SET/icon_256x256@2x.png"
render 512 "$APP_ICON_SET/icon_512x512.png"
render 1024 "$APP_ICON_SET/icon_512x512@2x.png"
render 1024 "$OUTPUT_PREVIEW"

/usr/bin/sips -g pixelWidth -g pixelHeight -g hasAlpha "$OUTPUT_PREVIEW"
echo "app_icon_set=$APP_ICON_SET"
echo "preview=$OUTPUT_PREVIEW"
