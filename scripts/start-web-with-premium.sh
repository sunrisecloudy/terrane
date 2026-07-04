#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREMIUM_REPO="${PREMIUM_REPO:-"$ROOT/../terrane-premium"}"
TERRANE_ADDR="${TERRANE_ADDR:-127.0.0.1:8780}"
TERRANE_APPS_DIR="${TERRANE_APPS_DIR:-apps}"
PREMIUM_PORT="${PREMIUM_PORT:-8788}"
PREMIUM_PORT_SCAN_END="${PREMIUM_PORT_SCAN_END:-8798}"
PREMIUM_LOG_DIR="${PREMIUM_LOG_DIR:-"$ROOT/.terrane-web"}"

premium_url_for_port() {
  printf 'http://127.0.0.1:%s' "$1"
}

is_real_premium() {
  local url="$1"
  local body
  body="$(curl -fsS --max-time 2 "$url/marketplace/premium-apps" 2>/dev/null || true)"
  [ -n "$body" ] || return 1

  node -e '
const fs = require("fs");
let body = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => body += chunk);
process.stdin.on("end", () => {
  try {
    const parsed = JSON.parse(body);
    const apps = parsed && parsed.ok === true && parsed.result && parsed.result.apps;
    if (!Array.isArray(apps)) process.exit(1);
    const premiumTodo = apps.find((app) => app && app.id === "premium-todo");
    if (!premiumTodo || premiumTodo.serverRequired !== true) process.exit(1);
    process.exit(0);
  } catch (_) {
    process.exit(1);
  }
});
' <<<"$body"
}

port_is_listening() {
  lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

find_real_premium_url() {
  local port url
  for ((port = PREMIUM_PORT; port <= PREMIUM_PORT_SCAN_END; port++)); do
    url="$(premium_url_for_port "$port")"
    if is_real_premium "$url"; then
      printf '%s\n' "$url"
      return 0
    fi
  done
  return 1
}

find_startable_premium_port() {
  local port
  for ((port = PREMIUM_PORT; port <= PREMIUM_PORT_SCAN_END; port++)); do
    if ! port_is_listening "$port"; then
      printf '%s\n' "$port"
      return 0
    fi
  done
  return 1
}

wait_for_real_premium() {
  local url="$1"
  local attempt
  for attempt in {1..80}; do
    if is_real_premium "$url"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

start_premium_if_needed() {
  local existing_url port url log
  existing_url="$(find_real_premium_url || true)"
  if [ -n "$existing_url" ]; then
    printf '%s\n' "$existing_url"
    return 0
  fi

  if [ ! -f "$PREMIUM_REPO/package.json" ]; then
    echo "Premium repo not found at $PREMIUM_REPO" >&2
    return 1
  fi

  port="$(find_startable_premium_port || true)"
  if [ -z "$port" ]; then
    echo "No free Premium port in ${PREMIUM_PORT}-${PREMIUM_PORT_SCAN_END}, and no real Premium API found." >&2
    return 1
  fi

  mkdir -p "$PREMIUM_LOG_DIR"
  log="$PREMIUM_LOG_DIR/premium-${port}.log"
  echo "Starting Terrane Premium from $PREMIUM_REPO on port $port (log: $log)" >&2
  (
    cd "$PREMIUM_REPO"
    env PORT="$port" PREMIUM_GOOGLE_FAKE="${PREMIUM_GOOGLE_FAKE:-1}" npm start
  ) >"$log" 2>&1 &

  url="$(premium_url_for_port "$port")"
  if ! wait_for_real_premium "$url"; then
    echo "Terrane Premium did not become ready at $url; see $log" >&2
    return 1
  fi
  printf '%s\n' "$url"
}

premium_url="$(start_premium_if_needed)"
echo "Using Terrane Premium at $premium_url" >&2

cd "$ROOT"
exec scripts/with-cargo-cache.sh cargo run -p terrane-host-web --bin terrane-web -- \
  --addr "$TERRANE_ADDR" \
  --apps "$TERRANE_APPS_DIR" \
  --premium-url "$premium_url"
