#!/usr/bin/env bash
# Start/stop terrane-web against one eval home for browser verification.
# The server holds the home's single-writer lock for its lifetime, so all CLI
# grading must happen BEFORE start; late grants go through the live admin
# console (POST /__terrane/admin/grants with X-Terrane-Admin: local-admin).
# Usage: serve-ui.sh start HOME PORT OUT_DIR   (prints PID, writes server.pid)
#        serve-ui.sh stop OUT_DIR HOME PORT
set -u
source "$(dirname "$0")/lib.sh" 2>/dev/null || {
  # allow standalone use without ROOT set
  harness_dir="$(cd "$(dirname "$0")" && pwd)"
  TERRANE_REPO="${TERRANE_REPO:-$(cd "$harness_dir/../../../.." && pwd)}"
  WEB_BIN="${WEB_BIN:-$TERRANE_REPO/host/web/target/debug/terrane-web}"
}

cmd="$1"
case "$cmd" in
  start)
    home="$2"; port="$3"; out_dir="$4"
    mkdir -p "$out_dir"
    env TERRANE_HOME="$home" "$WEB_BIN" --addr "127.0.0.1:$port" --no-live-reload \
      >"$out_dir/server.log" 2>&1 &
    pid=$!
    echo "$pid" > "$out_dir/server.pid"
    for _ in $(seq 1 20); do
      if curl -sf -o /dev/null "http://127.0.0.1:$port/apps"; then
        echo "$pid"
        exit 0
      fi
      if ! kill -0 "$pid" 2>/dev/null; then
        echo "ERROR: terrane-web exited early; see $out_dir/server.log" >&2
        exit 1
      fi
      sleep 0.5
    done
    echo "ERROR: terrane-web did not become ready on port $port" >&2
    kill "$pid" 2>/dev/null
    exit 1
    ;;
  stop)
    out_dir="$2"; home="$3"; port="$4"
    if [ -f "$out_dir/server.pid" ]; then
      pid="$(cat "$out_dir/server.pid")"
      kill "$pid" 2>/dev/null
      for _ in $(seq 1 20); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.5
      done
      kill -9 "$pid" 2>/dev/null
      rm -f "$out_dir/server.pid"
    fi
    # wait until both the port and the home lock are free before the next model
    for _ in $(seq 1 20); do
      if ! lsof -ti ":$port" >/dev/null 2>&1 && \
         [ -z "$(lsof -t "$home"/*.lock 2>/dev/null || true)" ]; then
        exit 0
      fi
      sleep 0.5
    done
    exit 0
    ;;
  *)
    echo "usage: serve-ui.sh start HOME PORT OUT_DIR | stop OUT_DIR HOME PORT" >&2
    exit 2
    ;;
esac
