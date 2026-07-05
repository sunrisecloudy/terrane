# Capability: `stream` — WebSocket/SSE subscriptions as recorded effects

New crate `rust/crates/terrane-cap-stream/`, namespace `stream`, registered in
`default_registry`. Lets an app subscribe to a live outbound feed (SSE or
WebSocket) — price tickers, chat relays, LLM token streams — and react per
message in its JS backend.

**Inbound-data principle (shared with [cap-webhook.md](cap-webhook.md)):**
messages arriving on a socket are *facts*. The edge records each one as an
event; replay folds the recorded messages and **never opens a socket**. The
subscription itself is the only effect; everything after it is observation.

## Design

`stream.open {app, name, request}` declares a desired subscription. The
`request` reuses the [cap-net-v2.md](cap-net-v2.md) request shape (URL +
headers + `sensitiveHeaders`, no method/body) plus `"kind": "sse" | "ws"`;
**redaction is identical to net-v2** — sensitive header values become
`«redacted»` before the `stream.opened` event is written, and
`{"$secret": "<name>"}` values resolve at the edge from
[cap-oauth-connections.md](cap-oauth-connections.md), with the marker (never
the secret) recorded verbatim.

The long-running host (web/mac) reconciles folded desired-state with live
sockets: after any fold that adds/removes an open stream, the edge connects or
disconnects. The CLI host has no long-running process — `open` folds, but
`doc.rs` says messages arrive only while a listening host runs (same stance as
webhook).

Per message, the edge commits `stream.message` and then invokes the app's
backend verb through `host.run` — the same post-commit dispatch as webhook —
so the backend's reaction lands as ordinary `kv.*` events (Option A) and
replay re-runs neither sockets nor JS.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `stream.open` | args `app, name, verb, request_json` → validate + redact purely; emits `stream.opened` (pure decision — the *connection* is live reconciliation, not a one-shot effect) |
| Command | `stream.close` | pure; emits `stream.closed {reason: "requested", by: "app"}`; edge tears down on fold |
| Event | `stream.opened` | `{app, name, verb, kind, request_json_redacted}` |
| Event | `stream.message` | `{app, name, seq, data_kind, data, data_is_base64, data_hash, data_size, received_at}` — recorded by the edge via a host-only ingest command (webhook pattern) |
| Event | `stream.reopened` | `{app, name, seq_before, attempt}` — marks a reconnect; messages lost during the gap were never facts and are not invented |
| Event | `stream.closed` | `{app, name, reason, by}` — `by: "app" \| "remote" \| "host"` |
| Resource | `stream.list()` | pure read: `[{name, kind, verb, lastSeq, status}]` |
| (reacts) | `app.removed` | drop the app's streams; edge disconnects on fold |

Fold keeps `app → name → StreamMeta {verb, kind, request_redacted, last_seq,
open}` — `seq` is a per-stream monotonic counter assigned by the edge at
record time and checked monotonic in fold (a typed error on regression keeps
replay honest).

## Reconnect policy

**Auto-reconnect with exponential backoff** (1 s doubling to a 60 s cap,
jittered), forever, until `stream.close` or `app.removed`. Every successful
reconnect records `stream.reopened` so the log is explicit that a gap may
exist — apps that need gap-free feeds must resync from the source (e.g. an
initial [net-v2](cap-net-v2.md) request) when they fold/observe a `reopened`.
For SSE the edge sends `Last-Event-ID` when the server provided ids —
best-effort gap narrowing, never a correctness claim.

## Message sizes, rates, and the blob CAS

- Per net-v2/[cap-blob.md](cap-blob.md): text messages ≤ 256 KiB inline;
  larger or binary → blob CAS with `data_kind: "blob"` + hash/size and a
  `__stream__/<app>/<name>/<seq>` blob link. Hard cap **8 MiB** per message —
  above that the edge closes the stream (`stream.closed {reason:
  "message-too-large"}`) rather than record a truncated fact.
- Rate cap: sustained > 20 messages/s over 10 s → edge closes with
  `reason: "rate-exceeded"`. Facts are never silently dropped; the stream as a
  whole is refused.
- **Log growth:** a chatty stream writes the log at message rate. This is the
  first cap whose normal operation makes compaction non-optional at scale —
  see cap-compaction.md for the snapshot/truncate story; until it lands,
  `doc.rs` states the growth honestly and recommends webhook or polling for
  high-frequency feeds.

## Security & permissions

- Grant resource `stream` (namespace-v1): "maintain live outbound
  WebSocket/SSE connections; every received message is recorded."
- SSRF stance identical to net-v2: `http(s)`/`ws(s)` schemes only, deny
  `169.254.169.254`, private/loopback ranges allowed (local-first is the
  point). Scheme downgrade on redirect refused.
- `describe()` prints host+path (no query string), kind, seq — never headers.

## Limits (documented in `doc.rs`)

- ≤ 16 open streams per app; `name` ≤ 128 chars, `[a-z0-9-_]` only.
- Message ≤ 8 MiB (blob offload above 256 KiB); rate ≤ 20/s sustained.

## Implementation plan

1. **Crate `terrane-cap-stream`:** manifest, decide (open/close/host-only
   ingest with seq check + size routing computed purely), fold,
   `message_event()`/`reopened_event()`/`closed_event()` constructors,
   describe, `doc.rs`.
2. **Edge reconciler:** `terrane-host/src/stream_edge.rs` — desired-state
   diff after fold, SSE + WS clients, backoff loop, ingest dispatch, blob
   offload (reuses the [cap-blob.md](cap-blob.md) CAS module), post-commit
   backend verb dispatch shared with webhook. Web host owns it; mac host
   reuses the embedded server's runtime; CLI documents unavailability.
3. **App surface:** `APP_API.md` — `resources: ["stream"]`, verb contract
   (verb receives the message JSON), `stream.list`; scaffold recipe mention.
4. **Tests:** engine (`terrane-core/tests/cap/stream.rs`): open/redaction,
   seq monotonicity, reopened/closed folds, replay identity, app.removed.
   E2e (`terrane-host/tests/cap/stream.rs`): loopback SSE + WS test servers —
   messages → events → backend verb, reconnect marks `reopened`, size/rate
   closes. Default-run (loopback only).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Sending on the socket (subscribe-only; a duplex protocol surface is a
different contract), gap-free delivery guarantees, per-message filtering at
the edge, internet-exposed *inbound* sockets (see
[cap-webhook.md](cap-webhook.md) exposure stance), stream multiplexing.

## Decisions to confirm

- **Auto-reconnect with backoff + `stream.reopened` gap marker** —
  *recommend: as specced* (streams exist to be up; the gap event keeps the log
  honest) — *alternative:* fail-stop on disconnect and make the app re-`open`;
  simpler edge, but every app rebuilds the same backoff loop in JS.
- **Rate/size violations close the stream instead of dropping messages** —
  *recommend: close* (a log that silently skipped facts is worse than no
  stream) — *alternative:* drop + a `stream.dropped {count}` marker event;
  keeps the stream up but makes "recorded = received" false.
- **`stream.open` is a pure decision, connection = reconciliation** —
  *recommend: as specced* (desired state in the log, liveness at the edge —
  survives host restarts for free) — *alternative:* `Decision::Effect` that
  connects once; restart semantics then need a second mechanism anyway.
