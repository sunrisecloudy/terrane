#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${MACOS_CERTIFICATE_P12_BASE64:?MACOS_CERTIFICATE_P12_BASE64 is required}"
: "${MACOS_CERTIFICATE_PASSWORD:?MACOS_CERTIFICATE_PASSWORD is required}"

CERTIFICATE_PATH="$RUNNER_TEMP/terrane-developer-id.p12"
KEYCHAIN_PATH="$RUNNER_TEMP/terrane-signing.keychain-db"
KEYCHAIN_PASSWORD="$(openssl rand -hex 24)"

printf '%s' "$MACOS_CERTIFICATE_P12_BASE64" | base64 --decode > "$CERTIFICATE_PATH"
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security import "$CERTIFICATE_PATH" \
  -P "$MACOS_CERTIFICATE_PASSWORD" \
  -A \
  -t cert \
  -f pkcs12 \
  -k "$KEYCHAIN_PATH"
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "$KEYCHAIN_PASSWORD" \
  "$KEYCHAIN_PATH"
security list-keychains -d user -s "$KEYCHAIN_PATH" login.keychain-db

identity="$(security find-identity -v -p codesigning "$KEYCHAIN_PATH" |
  sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' |
  head -n 1)"
if [[ -z "$identity" ]]; then
  printf 'imported certificate does not contain a Developer ID Application identity\n' >&2
  exit 1
fi
printf 'MACOS_SIGNING_IDENTITY=%s\n' "$identity" >> "$GITHUB_ENV"
printf 'MACOS_SIGNING_KEYCHAIN=%s\n' "$KEYCHAIN_PATH" >> "$GITHUB_ENV"
