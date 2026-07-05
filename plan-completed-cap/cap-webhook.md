# Capability: `webhook` — inbound HTTP as recorded facts

New crate `rust/crates/terrane-cap-webhook/`, namespace `webhook`, registered
in `default_registry`. Lets an app receive HTTP deliveries from other software
(git forges, payment providers, home-automation boxes) and react to them in its
JS backend.

**Inbound-data principle (shared with [cap-stream.md](cap-stream.md)):**
external inputs are *facts*. The edge records each delivery as an event; replay
folds the recorded deliveries and **never re-listens**. A webhook is not an
effect we perform — it is an observation we commit.

## Design

`webhook.register {app, name, verb}` asks the host to allocate a route on its
long-running HTTP listener:

```
POST /hook/<app>/<name>/<token>
```

- `token` is an unguessable 128-bit hex string minted **at the edge** (decide
  stays pure; the runner mints and returns the capability-owned
  `webhook.registered` event, same runner-emits-event pattern as `net`/`blob`).
  The token is part of the event so replay rebuilds the route table — the log
  is already the local plaintext source of truth, and the URL is a capability,
  not a credential for anything else.
- Each delivery: the listener matches the route, verifies the token
  (constant-time compare against folded state), then dispatches a host-only
  ingest command whose decide validates + shapes the payload and emits
  `webhook.received`. After commit, the host invokes the registered backend
  `verb` through the ordinary `host.run` path (js-runtime), so the backend's
  reaction is recorded as ordinary `kv.*` events (Option A) — replay re-runs
  neither the listener nor the JS.
- The caller gets `202 Accepted` as soon as the event is committed; the backend
  runs after. Apps cannot shape the HTTP response in v1 (see non-goals).

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `webhook.register` | args `app, name, verb` → `Decision::Effect(Effect::WebhookRegister)`; runner mints token, emits `webhook.registered` |
| Command | `webhook.rotate` | args `app, name` → same effect shape; runner mints a fresh token, emits `webhook.rotated`; old URL 404s immediately |
| Command | `webhook.unregister` | pure; emits `webhook.unregistered` |
| Command (host-only) | `webhook.ingest` | dispatched by the listener, never app/CLI-callable; decide checks token + caps, emits `webhook.received` (+ `blob.stored` on offload) |
| Event | `webhook.registered` / `webhook.rotated` | `{app, name, verb, token}` — fold upserts the route table |
| Event | `webhook.unregistered` | `{app, name}` |
| Event | `webhook.received` | `{app, name, method, headers, body_kind, body, body_is_base64, body_hash, body_size, received_at}` |
| Resource | `webhook.list()` | pure read: `[{name, verb, url_path}]` (path includes token — the app owns its own hooks) |
| (reacts) | `app.removed` | drop the app's routes; live listener stops matching on next fold |

Fold keeps `app → name → HookMeta {verb, token}` plus a per-hook delivery
counter for `stat`-style reads.

## Headers, bodies, and the blob CAS

- **Headers filtered per [cap-net-v2.md](cap-net-v2.md) rules:** the net-v2
  built-in sensitive list (`authorization`, `cookie`, `x-api-key`,
  `*-token`/`*-secret`, …) is redacted to `«redacted»` before the event is
  written. HMAC signature headers (`x-hub-signature-256` and friends) carry a
  MAC, not a key, and are recorded verbatim — that is the supported
  verification style (the app verifies the recorded MAC against a
  `{"$secret"}`-held key via [cap-oauth-connections.md](cap-oauth-connections.md)).
- **Bodies per net-v2:** ≤ 256 KiB text inlines in the event; larger or binary
  bodies go to the blob CAS ([cap-blob.md](cap-blob.md)) with
  `body_kind: "blob"` + hash/size/mime and a `__webhook__/<app>/<name>/<seq>`
  blob link. Hard cap **32 MiB**; oversized deliveries get `413` and no event.
- `received_at` is edge wall-clock at record time — a recorded fact, so replay
  identity holds.

## Ordering, dedup, retries

- **Ordering:** log order is delivery order — the single listener serializes
  commits, so per-hook deliveries fold in arrival order. No per-hook seq
  needed in v1 (the event's log position is the seq).
- **Dedup: none.** Semantics are at-least-once (senders retry; a crash between
  commit and backend-run means the backend may see a delivery it half-handled).
  Apps dedup with sender idempotency keys in kv — documented in `doc.rs`.

## Hosts

- **web host:** owns the listener; routes mount beside the existing app/blob
  routes.
- **mac host:** proxies `/hook/*` to its embedded web server — same behavior.
- **CLI host:** no long-running process; `register` still folds (state is
  host-independent) but `doc.rs` and the CLI print that deliveries arrive only
  while a listening host runs.
- **Exposure: local-network only in v1.** The listener binds the host's
  existing LAN interface. Internet exposure (tunnel/relay) is a non-goal — it
  belongs to the future sync/relay transport, not to this cap.

## Security & permissions

- Grant resource `webhook` (namespace-v1) described as "receive inbound HTTP
  from other software on your network" — explicit prompt, since a hook is an
  open door.
- Token compare is constant-time; unknown routes and bad tokens both 404
  identically (no route enumeration).
- Rate cap: 60 deliveries/min per hook; above → `429`, no event (a refused
  delivery is not a fact we observed).

## Limits (documented in `doc.rs`)

- ≤ 32 hooks per app; `name` ≤ 128 chars, `[a-z0-9-_]` only.
- Body ≤ 32 MiB (blob offload above 256 KiB); headers ≤ 32 KiB total.

## Implementation plan

1. **Interface:** add `Effect::WebhookRegister { app, name, verb }` to
   `terrane-cap-interface::abi` (runner mints token, returns event).
2. **Crate `terrane-cap-webhook`:** manifest, decide (`register`/`rotate`/
   `unregister`/host-only `ingest` with header filter + body routing computed
   purely like `blob.put`), fold, `registered_event()`/`received_event()`
   constructors, describe (never prints token), `doc.rs`.
3. **Edge:** `WebhookRegister` arm in `EdgeRunner::run`
   (`terrane-host/src/edge.rs`); listener route in the web host + mac proxy;
   post-commit backend dispatch through the existing invoke path; mark
   `webhook.ingest` host-only in the request router.
4. **App surface:** `APP_API.md` — manifest `resources: ["webhook"]`, the
   backend verb contract (verb receives the delivery JSON), `webhook.list`.
5. **Tests:** engine (`terrane-core/tests/cap/webhook.rs`): register/rotate/
   ingest fold + replay identity, header redaction, token check, app.removed.
   E2e (`terrane-host/tests/cap/webhook.rs`): loopback POST → event → backend
   verb ran → kv changed; rotation kills old URL; 429/413 paths. Default-run
   (loopback only).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

App-controlled HTTP responses, internet exposure/tunnels (future sync/relay
transport), delivery retries/dead-lettering, inbound auth schemes beyond
token-path + recorded MACs, receiving email (see [cap-common.md](cap-common.md)
non-goals).

## Decisions to confirm

- **Token in the event log** — *recommend: yes, record it* (replay must rebuild
  routes; the log is already the local trust boundary) — *alternative:* keep
  tokens in the [cap-oauth-connections.md](cap-oauth-connections.md) secret
  store and record only a token hash; costs an edge lookup on every fold-driven
  route rebuild and complicates sync.
- **Signature headers recorded verbatim while secret-valued headers are
  redacted** — *recommend: as specced* (MACs are safe to record; secret-valued
  headers like `x-gitlab-token` are not, so those senders must use the token
  path URL itself) — *alternative:* per-hook `recordHeaders` allowlist.
- **`202` before the backend runs** — *recommend: yes* (commit-then-react keeps
  the listener fast and the fact durable) — *alternative:* run the verb inline
  and return its output; couples sender latency to app JS and invites timeouts.
