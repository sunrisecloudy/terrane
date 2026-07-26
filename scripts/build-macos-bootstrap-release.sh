#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-}"
OUTPUT="${2:-$ROOT/artifacts/macos-bootstrap/${VERSION}}"
BASE_URL="${TERRANE_UPDATE_BASE_URL:-}"
SIGNING_KEY="${TERRANE_UPDATE_SIGNING_KEY:-}"

if [[ -z "$VERSION" || -z "$BASE_URL" || -z "$SIGNING_KEY" ]]; then
  cat >&2 <<'USAGE'
usage: TERRANE_UPDATE_BASE_URL=https://github.com/OWNER/REPO/releases/download/TAG \
       TERRANE_UPDATE_SIGNING_KEY=/secure/path/update-signing-key.pem \
       scripts/build-macos-bootstrap-release.sh VERSION [OUTPUT_DIRECTORY]
USAGE
  exit 2
fi
if [[ ! -f "$SIGNING_KEY" ]]; then
  echo "update signing key does not exist: $SIGNING_KEY" >&2
  exit 1
fi
PUBLIC_KEY="$(
  openssl pkey -in "$SIGNING_KEY" -pubout -outform DER \
    | tail -c 32 \
    | xxd -p -c 64
)"
if ! grep -q "$PUBLIC_KEY" \
  "$ROOT/host/macos/BootstrapSources/BootstrapConfiguration.swift"
then
  echo "release signing key does not match the public key embedded in the bootstrap" >&2
  exit 1
fi

mkdir -p "$OUTPUT"
TERRANE_CARGO_DIR="$(
  "$ROOT/scripts/with-cargo-cache.sh" env \
    | sed -n 's/^CARGO_TARGET_DIR=//p'
)"
if [[ -z "$TERRANE_CARGO_DIR" ]]; then
  echo "could not resolve the shared Terrane Cargo target directory" >&2
  exit 1
fi

env \
  MACOSX_DEPLOYMENT_TARGET=13.0 \
  CMAKE_TOOLCHAIN_FILE="$ROOT/host/macos/cmake/static-host.cmake" \
  "$ROOT/scripts/with-cargo-cache.sh" \
  cargo build -p terrane-host -p terrane-host-web --release --locked

HTTP_ARCHIVE="$(
  find "$TERRANE_CARGO_DIR/release/build" \
    -path '*/out/build/vendor/cpp-httplib/libcpp-httplib.a' \
    -type f -print \
    | sort \
    | tail -n 1
)"
if [[ -z "$HTTP_ARCHIVE" ]]; then
  echo "llama-cpp-sys did not produce libcpp-httplib.a" >&2
  exit 1
fi
cp "$HTTP_ARCHIVE" "$TERRANE_CARGO_DIR/release/libterrane_host_httplib.a"

DERIVED="$OUTPUT/DerivedData"
(
  cd "$ROOT/host/macos"
  xcodegen generate
  xcodebuild \
    -project Terrane.xcodeproj \
    -scheme TerraneHost \
    -configuration Release \
    -derivedDataPath "$DERIVED/runtime" \
    TERRANE_SKIP_RUST_BUILD=1 \
    TERRANE_RUST_LIB_DIR="$TERRANE_CARGO_DIR/release" \
    PRODUCT_NAME=Terrane \
    CODE_SIGN_IDENTITY=- \
    clean build
  xcodebuild \
    -project Terrane.xcodeproj \
    -scheme TerraneBootstrap \
    -configuration Release \
    -derivedDataPath "$DERIVED/bootstrap" \
    PRODUCT_NAME=Terrane \
    CODE_SIGN_IDENTITY=- \
    clean build
)

RUNTIME_APP="$DERIVED/runtime/Build/Products/Release/Terrane.app"
BOOTSTRAP_APP="$DERIVED/bootstrap/Build/Products/Release/Terrane.app"
RUNTIME_EXECUTABLE="$RUNTIME_APP/Contents/MacOS/$(
  /usr/bin/plutil -extract CFBundleExecutable raw "$RUNTIME_APP/Contents/Info.plist"
)"
BOOTSTRAP_EXECUTABLE="$BOOTSTRAP_APP/Contents/MacOS/$(
  /usr/bin/plutil -extract CFBundleExecutable raw "$BOOTSTRAP_APP/Contents/Info.plist"
)"

/usr/bin/codesign --verify --deep --strict --verbose=2 "$RUNTIME_APP"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$BOOTSTRAP_APP"
if /usr/bin/codesign -d --entitlements :- "$RUNTIME_APP" 2>/dev/null \
  | grep -q 'com.apple.security.get-task-allow'
then
  echo "runtime release must not contain the get-task-allow entitlement" >&2
  exit 1
fi
if /usr/bin/codesign -d --entitlements :- "$BOOTSTRAP_APP" 2>/dev/null \
  | grep -q 'com.apple.security.get-task-allow'
then
  echo "bootstrap release must not contain the get-task-allow entitlement" >&2
  exit 1
fi
[[ "$(/usr/bin/lipo -archs "$RUNTIME_EXECUTABLE")" == "arm64" ]]
[[ "$(/usr/bin/lipo -archs "$BOOTSTRAP_EXECUTABLE")" == "arm64" ]]
/usr/bin/vtool -show-build "$RUNTIME_EXECUTABLE" | grep -q 'minos 13.0'
/usr/bin/vtool -show-build "$BOOTSTRAP_EXECUTABLE" | grep -q 'minos 13.0'

"$ROOT/scripts/package-bootstrap-runtime.mjs" \
  --app "$RUNTIME_APP" \
  --output "$OUTPUT" \
  --version "$VERSION" \
  --base-url "$BASE_URL" \
  --signing-key "$SIGNING_KEY"
"$ROOT/scripts/package-bootstrap-dmg.sh" \
  "$BOOTSTRAP_APP" \
  "$OUTPUT/Terrane-Bootstrap-arm64.dmg"

(
  cd "$OUTPUT"
  /usr/bin/shasum -a 256 Terrane-Bootstrap-arm64.dmg >> SHA256SUMS
  /usr/bin/shasum -a 256 -c SHA256SUMS
)

rm -rf "$DERIVED"
echo "release_assets=$OUTPUT"
