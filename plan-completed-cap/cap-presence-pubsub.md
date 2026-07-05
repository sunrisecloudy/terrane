# Capability: `presence` — ephemeral realtime channels between replicas

New crate `rust/crates/terrane-cap-presence/`, namespace `presence`, registered
in `default_registry`. Gives apps live, low-latency signals between replicas
viewing the same shared app (see `cap-share-invite.md`): who's here, cursor
positions, typing indicators, "player 2 moved" — the class of data that is
worthless one second later.

## Locked decision

**Presence messages never touch the event log.** This is the one capability
that is deliberately transient: a cursor position has no replay value and
would grow the log without bound (a moving mouse is ~60 events/second). The
capability crate records only **durable facts** — channel definitions and
their limits — and folds those; the messages themselves are host-to-host
frames over the sync-v2 transport, delivered best-effort and forgotten.
Replay identity is trivially safe because replay never sees a message. If a
signal turns out to deserve durability, the app writes it to `kv`/`crdt` like
any other fact — presence is not a side door around the log.

## Transport

WebSocket upgrade on the sync-v2 listener (`cap-sync-v2.md`): route
`GET /sync/presence` on the web host, authenticated with the same bearer token
as every other `/sync/*` route. tungstenite is already a dependency and the
web host already serves a WebSocket (`/__terrane/stt/pcm`), so this is a
proven path, not new machinery. Each paired-and-connected peer holds one
socket; frames are JSON `{app, channel, from_peer, payload}`. The host fans a
published frame out to every connected peer whose replica has a `share` grant
for that app (read suffices — presence is observation). No store-and-forward:
a peer that is offline misses the frame, full stop.

## Capability surface (durable facts only)

| Command | Args | Decision |
| --- | --- | --- |
| `presence.channel.define` | `app, channel, max_payload?, max_rate?` | Pure: validate names/limits, emit `presence.channel.defined`. Idempotent redefine updates limits. |
| `presence.channel.drop` | `app, channel` | Pure: emit `presence.channel.dropped`. |

| Event | Payload | Fold |
| --- | --- | --- |
| `presence.channel.defined` | `{app, channel, max_payload, max_rate}` | upsert `app → channel → limits` |
| `presence.channel.dropped` | `{app, channel}` | drop channel |
| (reacts) `app.removed` | — | drop the app's channels |

Publishing to an undefined channel auto-defines it with default limits on
first use (`presence.channel.defined` is emitted once, by the publish path's
recorded companion dispatch) — apps shouldn't need ceremony for a cursor.

### Resource methods (JS backend: `ctx.resource.presence`)

| Method | Semantics |
| --- | --- |
| `publish(channel, json)` | `Decision::TransientEffect(Effect::PresencePublish {…})` — live fan-out, **never recorded** (same shape as `net.get`) |
| `peers(channel)` | connected peers currently seen on the channel (edge live read, like sysinfo) |

Grant resource: `presence` namespace-v1 with `publish`/`subscribe`, described
as "ephemeral realtime signals to replicas sharing this app" — flows through
the existing permission prompts.

### UI surface (`window.terrane`)

Mirrors exactly how `onDocument`/`onTheme` work in the web shell today
(`host/web/src/js/terrane_shim.js` + `app_shell.js`): the shell posts
`postMessage` frames into the app iframe and the shim keeps a subscriber list.

- Shell → app: `{type: "terrane:presence", channel, from, payload}`; the shim
  verifies `event.source === window.parent`, then notifies subscribers.
- `window.terrane.onPresence(channel, cb)` — push `cb` onto a per-channel
  subscriber array, return an unsubscriber (the `unsubscriber()` helper the
  shim already has). Unlike `onDocument` there is no replayed current value —
  presence has no "current state" to fire immediately.
- `window.terrane.publishPresence(channel, payload)` — app → shell message
  carrying the per-load bridge nonce (like `terrane:document:set`); the shell
  forwards to the host, which fans out. macOS host mirrors via its existing
  shim-parity bridge.

## Delivery contract and limits (documented in `doc.rs`)

- **Best-effort, at-most-once, unordered across peers.** No acks, no retries,
  no queueing for offline peers. Apps must treat every message as optional.
- Payload ≤ 16 KiB (default; channel-definable down, never above 64 KiB).
- Rate: ≤ 20 msgs/sec per (app, channel, publisher) default — exceeding drops
  newest frames and surfaces a typed error to the publisher, it never queues.
- ≤ 64 channels per app; channel names ≤ 128 chars.
- Fan-out only to peers holding a `share` grant for the app; no grant, no
  frames, in either direction.

## Implementation plan

1. **Interface:** add `Effect::PresencePublish { app, channel, payload }` to
   `terrane-cap-interface::abi` (transient-only — no event constructor ever
   carries a message).
2. **Crate `terrane-cap-presence`:** channel-definition decide/fold/describe,
   resource methods, grant resource, `doc.rs` with the delivery contract;
   register in `default_registry`.
3. **Host hub** (`terrane-host/src/presence.rs`): in-memory channel hub —
   local subscribers + connected peer sockets, rate limiting, share-grant
   check against folded state; wire `Effect::PresencePublish` into
   `EdgeRunner`.
4. **Web host:** `/sync/presence` WebSocket route (bearer-checked); shell
   `terrane:presence` push into app frames + nonce-checked publish path;
   `onPresence`/`publishPresence` in `terrane_shim.js`; `APP_API.md` docs.
5. **macOS host:** same two shim methods over the existing native bridge.
6. **Tests:** engine tests `terrane-core/tests/cap/presence.rs` (define/drop
   fold, replay identity, and the invariant that publish produces **zero**
   event records); e2e `terrane-host/tests/cap/presence.rs` — two temp homes,
   loopback WebSocket: publish crosses, offline peer misses it, rate limit
   drops, unshared app gets nothing (default-run, loopback only).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Durable pub/sub (that's `kv`/`crdt` + sync), delivery guarantees or ordering,
message history/late-join catch-up, presence across unpaired replicas or the
internet (bounded by sync-v2's LAN transport), backend-side `onMessage` (the
JS backend is request-scoped; subscriptions are a UI-layer surface in v1).

## Decisions to confirm

- **Auto-define on first publish** — recommendation: yes, with default limits
  (zero ceremony). Alternative: require explicit `presence.channel.define`
  (stricter, but every app ships boilerplate).
- **Socket topology** — recommendation: one `/sync/presence` WebSocket per
  peer pair, multiplexing all apps/channels. Alternative: piggyback frames on
  a combined sync+presence socket (couples the sync loop's lifecycle to UI
  liveness; rejected for v1).
- **Backend subscriptions** — recommendation: UI-only in v1 (`onPresence`),
  backend can publish + poll `peers()`. Alternative: invoke the backend per
  frame (turns a 60 Hz cursor into 60 Hz QuickJS dispatches; rejected).
