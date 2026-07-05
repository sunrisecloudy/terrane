# Capability: `net` v2 — full HTTP

An **extension of the existing `net` crate**, not a new namespace. Today `net`
is GET-only with a bare URL. v2 adds methods, headers, bodies, timeouts, and
redirect control, while keeping `net.fetch`/`net.get` byte-for-byte
backward-compatible (old events still fold; old apps keep working).

## Locked decision

**Redact on record.** Requests are recorded (so replay is documented and
request-keyed), but sensitive header *values* are replaced with the marker
`«redacted»` before the event is written. A built-in list —
`authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
`api-key`, plus any header matching `*-token`/`*-secret` (case-insensitive) —
is always redacted; the request may declare additional names in
`sensitiveHeaders`. Replay folds the recorded **response**, never re-sends the
request, so redaction can never break replay-identity. The request schema
reserves the `{"$secret": "<name>"}` value shape (rejected in v2 with a clear
error) so a future host secret store / oauth-connections cap can slot in
without an event-format change.

## Request spec

One canonical JSON object (the single string arg of the new command):

```jsonc
{
  "method": "POST",                 // GET|POST|PUT|PATCH|DELETE|HEAD (default GET)
  "url": "https://api.example.com/items",
  "headers": { "content-type": "application/json", "authorization": "Bearer …" },
  "body": "…",                      // string, or {"$base64": "…"} for binary, or absent
  "sensitiveHeaders": ["x-internal-auth"],   // extra redaction, optional
  "timeoutMs": 30000,               // default 30_000, max 120_000
  "redirect": "follow",             // follow (≤5 hops) | manual | deny; default follow
  "responseBody": "auto"            // auto | inline | blob; default auto
}
```

Canonicalization (in `decide`, pure): lowercase header names, sort keys,
serialize with sorted-key canonical JSON → `requestKey = sha256(canonical)`.
`requestKey` is the state key, so identical requests overwrite (matching
today's URL-keyed `NetState.fetches` semantics).

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `net.request` | args `app, request_json` → `Decision::Effect(Effect::HttpRequest { app, request })` — **recorded** |
| Resource | `net.call(request_json)` | same effect wrapped in `Decision::TransientEffect` — live, never recorded (parallel to today's `net.get`) |
| Command (kept) | `net.fetch` | unchanged GET path, still emits `net.fetched` |
| Resource (kept) | `net.get(url)` | unchanged transient GET |
| Event (new) | `net.responded` | see below |
| Event (kept) | `net.fetched` | folds exactly as today |

`net.responded` payload (borsh):

```text
{ app, request_key, request_json_redacted, status,
  response_headers,        // filtered: content-type, content-length, etag,
                           // last-modified, location, cache-control only
  body_kind,               // "inline" | "blob"
  body,                    // inline: the string (binary → base64 with flag)
  body_is_base64, body_hash, body_size, body_mime }
```

Fold: `NetState` gains `requests: BTreeMap<AppId, BTreeMap<RequestKey, RecordedResponse>>`;
`app.removed` clears it like today.

## Response bodies and the blob CAS

- `responseBody: "auto"` (default): bodies **≤ 256 KiB** and text-typed are
  inlined in the event; larger or binary bodies are written into the blob CAS
  (`blobs.sqlite3`, same edge module as `cap-blob`) and the event carries
  `body_kind: "blob"` + `body_hash/body_size/body_mime`. Hard cap **32 MiB**
  (typed error above).
- Reading a blob body from an app requires the `blob` resource grant; the
  runner also emits a `blob.link`-equivalent `blob.stored` event named
  `__net__/<request_key>` so the bytes are reachable through the normal blob
  surface and participate in refcount GC. If the app has no `blob` grant the
  request still succeeds — the app just can't read the body until granted
  (error message says exactly that).
- `"inline"` forces inline (errors above 8 MiB); `"blob"` forces CAS.

## Edge behavior

- Reuse the existing HTTP client used for `Effect::HttpGet` in
  `terrane-host/src/edge.rs`; add the `Effect::HttpRequest` arm.
- Timeout enforced per request; connect+read within `timeoutMs`.
- Redirects: `follow` caps at 5 hops and refuses scheme downgrades
  (https→http); `manual` returns the 3xx with its `location` header; `deny`
  errors on any 3xx.
- **SSRF guard:** scheme must be `http`/`https`; deny requests resolving to
  the cloud-metadata address `169.254.169.254` outright. Private/loopback
  ranges stay *allowed* (Terrane is a local-first platform; localhost APIs are
  a feature) — this line is documented in `doc.rs` so it's an explicit choice,
  not an accident.
- No automatic retries in v2 (a retry would re-run an effect the log already
  has an opinion about); callers retry by issuing a new request.

## Security notes

- Redaction happens **before** `EventRecord` construction — the plaintext
  header value never reaches persistence, `describe()`, or MCP event dumps.
- `describe()` for `net.responded` prints method, host+path (no query string —
  query strings often carry tokens), status, body size.
- Grants: the existing `net` namespace grant covers v2; the grant description
  is updated to say "full HTTP requests" so the permission prompt is honest.

## Implementation plan

1. **Interface:** add `Effect::HttpRequest { app, request: String }` (canonical
   JSON) to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-net`:** `request.rs` — request spec parse/validate/
   canonicalize/redact + `requestKey`; new command/resource in `manifest()` and
   `decide`; `responded_event()` constructor; fold + describe; `doc.rs` update
   (limits, redaction list, SSRF line, `$secret` reservation).
3. **Edge:** `HttpRequest` arm in `EdgeRunner::run` — perform request, apply
   redirect policy, choose inline vs CAS body, return `[net.responded]` (+
   `blob.stored` when offloaded). Depends on cap-blob step 3 (CAS module).
4. **App surface:** `APP_API.md` — `ctx.resource.net.call(requestJson)` and the
   recorded `net.request` verb pattern; scaffold recipe mention.
5. **Tests:** engine tests (`terrane-core/tests/cap/net.rs` extension):
   canonicalization stability, redaction (built-in + declared), fold/replay
   identity for both event kinds, back-compat fold of legacy `net.fetched`.
   E2e (`terrane-host/tests/cap/net.rs`): local test server (bind
   `127.0.0.1:0`) covering POST+headers+body, redirect policies, timeout,
   binary→CAS offload — runs by default since it's loopback-only; the
   real-internet cases stay `#[ignore]` like today.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.
