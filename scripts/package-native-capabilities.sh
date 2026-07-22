#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=scripts/cargo-cache-env.sh
. "$ROOT/scripts/cargo-cache-env.sh" --quiet

OUTPUT="${1:-$ROOT/target/native-capabilities}"
: "${TERRANE_CAP_SIGNING_KEY_HEX:?set TERRANE_CAP_SIGNING_KEY_HEX to a 32-byte Ed25519 seed in hex}"

cd "$ROOT"
cargo build -p terrane-cap-worker --bins --release --locked
case "$(uname -s)" in
  Darwin) PLATFORM="macos" ;;
  Linux) PLATFORM="linux" ;;
  *) printf 'unsupported packaging platform: %s\n' "$(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64) ARCHITECTURE="aarch64" ;;
  aarch64 | x86_64) ARCHITECTURE="$(uname -m)" ;;
  *) printf 'unsupported packaging architecture: %s\n' "$(uname -m)" >&2; exit 1 ;;
esac
"$CARGO_TARGET_DIR/release/terrane-cap-bundle" \
  --worker "$CARGO_TARGET_DIR/release/terrane-cap-worker" \
  --output "$OUTPUT" \
  --platform "$PLATFORM" \
  --architecture "$ARCHITECTURE"

printf 'native capability bundles: %s\n' "$OUTPUT"
