#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=scripts/cargo-cache-env.sh
. "$ROOT/scripts/cargo-cache-env.sh" --quiet

OUTPUT="${1:-$ROOT/target/native-capabilities}"
: "${TERRANE_CAP_SIGNING_KEY_HEX:?set TERRANE_CAP_SIGNING_KEY_HEX to a 32-byte Ed25519 seed in hex}"

cd "$ROOT"
cargo build -p terrane-cap-worker --bin terrane-cap-bundle --release --locked \
  --no-default-features --features packager

WORKERS="$(mktemp -d "${TMPDIR:-/tmp}/terrane-cap-workers.XXXXXX")"
trap 'rm -rf "$WORKERS"' EXIT
CAPABILITIES=(
  agent:cap-agent
  applescript:cap-applescript
  automation:cap-automation
  browser:cap-browser
  build:cap-build
  builder:cap-builder
  common:cap-common
  crdt:cap-crdt
  crypto:cap-crypto
  document:cap-document
  geo:cap-geo
  harness:cap-harness
  history:cap-history
  interop:cap-interop
  job:cap-job
  js-runtime:cap-js-runtime
  local-model:cap-local-model
  mcp:cap-mcp
  media:cap-media
  migration:cap-migration
  model:cap-model
  native:cap-native
  net:cap-net
  org:cap-org
  presence:cap-presence
  publish:cap-publish
  push:cap-push
  query:cap-query
  relational_db:cap-relational-db
  scheduler:cap-scheduler
  search:cap-search
  share:cap-share
  stream:cap-stream
  stt:cap-stt
  sync:cap-sync
  sysinfo:cap-sysinfo
  time:cap-time
  tts:cap-tts
  wasm-runtime:cap-wasm-runtime
  web-publish:cap-web-publish
  webhook:cap-webhook
)
for entry in "${CAPABILITIES[@]}"; do
  namespace="${entry%%:*}"
  feature="${entry#*:}"
  cargo build -p terrane-cap-worker --bin terrane-cap-worker --release --locked \
    --no-default-features --features "$feature"
  cp "$CARGO_TARGET_DIR/release/terrane-cap-worker" "$WORKERS/$namespace"
done
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
PACKAGER_ARGS=(
  --workers "$WORKERS"
  --output "$OUTPUT"
  --platform "$PLATFORM"
  --architecture "$ARCHITECTURE"
)
if [[ -n "${TERRANE_CAP_INDEX_BASE_URL:-}" ]]; then
  PACKAGER_ARGS+=(--download-base-url "$TERRANE_CAP_INDEX_BASE_URL")
fi
"$CARGO_TARGET_DIR/release/terrane-cap-bundle" \
  "${PACKAGER_ARGS[@]}"

printf 'native capability bundles: %s\n' "$OUTPUT"
