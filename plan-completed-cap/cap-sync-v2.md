# Sync v2 — continuous LAN sync over HTTP

Mostly **host work** (`terrane-host/src/sync.rs` grows up), plus one small new
crate `rust/crates/terrane-cap-sync/` for the durable session facts. Today sync
is manual and one-shot: file-based `terrane sync <app> --from <home>` and raw
TCP `terrane serve` / `sync <app> --peer <addr>`, both of which exchange **only
CRDT data** — Loro version vectors, then the deltas each side lacks, merged via
the recorded `crdt.merge` command. Non-CRDT events never cross homes at all
today; there is no foreign-event ordering to preserve, so v2 gets to define it.

## Locked decision

**HTTP on the existing web-host listener, discovered via mDNS, driven by a
long-poll loop.** The web host (`host/web`, tiny_http + tungstenite) already
listens, already checks bearer tokens (`host/web/src/http.rs`), and already
speaks WebSocket (`/__terrane/stt/pcm`). Sync v2 adds `/sync/*` routes there
instead of keeping the bespoke raw-TCP framing; the CLI grows
`terrane sync <app> --peer` pointing at HTTP and a `--watch` continuous mode.
Discovery is Bonjour/mDNS via the `mdns-sd` crate (pure Rust, no daemon),
advertising `_terrane-sync._tcp` with the replica peer id in the TXT record.

## Protocol — two channels per app

### Channel 1: CRDT (unchanged semantics, new carriage)

Exactly today's exchange, over HTTP: client POSTs its version vector
(`crdt_vv`), server replies with `crdt_export_from_vv` delta + its own vv,
client POSTs back the delta the server lacks. Both sides merge via `crdt.merge`
— recorded, replayable, convergent. `crdt.update` events remain the wire
format; nothing about the CRDT story changes.

### Channel 2: event-cursor exchange (new, allowlisted kinds)

For single-author facts (v1 allowlist: `kv.*` only) each side keeps a cursor
per `(peer, app)`: "I hold origin-peer P's events for app A up to origin-seq
S." Exchange: client sends its cursors, server replies with newer events in an
envelope `{origin_peer, origin_seq, kind, payload}` (origin_seq = the event's
seq in the origin log; `EventRecord` itself is only `{kind, payload}`, so the
envelope is transport metadata). Ingest is one dispatch of the new `sync.apply`
command per batch; decide validates monotonic seqs against folded cursors and
emits `sync.applied {peer, app, from_seq, to_seq}` followed by the foreign
events **verbatim, in arrival order**. Fold of `sync.applied` advances the
cursor, so replay rebuilds cursors with no network — the same
record-what-arrived shape as `crdt.merge`.

**Conflict stance (documented in `doc.rs`):** `kv` is last-writer-wins, and
"last" means local log order — foreign `kv.set`s fold in arrival order after
whatever this replica already wrote. `crdt` converges regardless of order.
Every other capability's events are replica-local facts and are **not synced**;
widening the allowlist is a per-cap decision, never a default.

### Blob pass

After the event pass, per `cap-blob.md`: collect hashes referenced by the
synced app's folded state, `GET /sync/<app>/blob/<hash>` for rows missing from
the local CAS. Events-before-blobs means a crashed sync leaves a typed
`BlobMissing` on read, healed by re-running.

## Capability surface (`sync` crate — session facts only)

| Command | Args | Decision |
| --- | --- | --- |
| `sync.pair` | `peer_hex, display_name` | Pure: emit `sync.peer.paired` (idempotent per peer). |
| `sync.unpair` | `peer_hex` | Pure: emit `sync.peer.unpaired`. |
| `sync.apply` | `peer_hex, app, from_seq, to_seq, batch_hex` | Validate cursor monotonicity; emit `sync.applied` + the foreign events verbatim. |

| Event | Payload | Fold |
| --- | --- | --- |
| `sync.peer.paired` | `{peer, display_name}` | upsert peer roster |
| `sync.peer.unpaired` | `{peer}` | drop peer |
| `sync.applied` | `{peer, app, from_seq, to_seq}` | advance `(peer, app)` cursor |

Queries: `sync.peers`, `sync.cursor <peer> <app>`. No app-facing resource —
apps never drive sync; the host does. The bearer token lives in
`$TERRANE_HOME/sync-tokens.json` at the edge, **never in the log** (secrets are
not events); only the pairing *fact* is recorded.

## Pairing and trust

`terrane serve` prints a one-time 6-digit code (5-minute TTL, single use). The
peer runs `terrane pair <addr-or-mdns-name> --code <code>`: it POSTs
`/sync/pair {code, peer_hex, display_name}`; the server verifies the code,
dispatches `sync.pair` **and** an `auth.grant` for the paired replica subject
(`replica:<peer_hex>` — see `cap-share-invite.md` for scoping), and returns a
random bearer token the client stores edge-side. Every subsequent `/sync/*`
request carries that token; the server maps token → peer and consults folded
auth/share state before serving or accepting anything.

## Continuous mode

`terrane sync --watch [app]` (and a background task in the web host): loop of
long-poll `GET /sync/<app>/wait?vv=<hex>&cursors=<json>` — the server answers
immediately if it has anything newer, else parks the request up to 30 s and
answers `204`. On answer, run both channels + blob pass, then re-poll.
Exponential backoff on connection failure: 1 s doubling to 60 s, reset on
success. mDNS re-resolution on each reconnect, so DHCP address churn heals.

## Limits

- Frame/batch cap 64 MiB (matches today's `MAX_FRAME`); event batches ≤ 5 000
  events, larger backlogs page through repeated `sync.apply` dispatches.
- One app per request pipeline, sequential merges (today's model) — parallel
  per-app sync is a later optimization, not v2.
- Pairing codes: ≤ 5 attempts, then the code burns.

## Implementation plan

1. **Crate `terrane-cap-sync`:** state (peer roster + cursors), `sync.pair` /
   `sync.unpair` / `sync.apply` decide + fold + describe, `doc.rs` with the
   conflict-stance table; register in `default_registry`.
2. **Envelope + batch codec** in `terrane-host/src/sync.rs`: borsh envelope,
   allowlist filter (`kv.*`), read-side extraction of an app's events with
   origin seqs from the local log.
3. **HTTP routes** in `host/web`: `/sync/pair`, `/sync/<app>/vv`,
   `/sync/<app>/delta`, `/sync/<app>/events`, `/sync/<app>/blob/<hash>`,
   `/sync/<app>/wait` — all bearer-checked via the existing helper; token
   store + one-time-code minting at the edge.
4. **Client loop:** `terrane pair`, `terrane sync <app> --peer <url>` (HTTP),
   `terrane sync --watch` with long-poll + backoff.
5. **mDNS:** advertise on `terrane serve`/web host start (`mdns-sd`), resolve
   in `terrane pair`/`--peer` when given a `.local` name; `terrane peers ls`.
6. **Deprecate raw TCP:** keep `run_serve`/`run_sync_peer` one release with a
   pointer, then remove.
7. **Tests:** engine tests `terrane-core/tests/cap/sync.rs` (cursor
   monotonicity, replay identity of `sync.applied` + foreign folds, kv LWW
   arrival order); e2e `terrane-host/tests/cap/sync.rs` — two temp homes over a
   loopback HTTP server: pair, diverge kv + crdt, converge, blob pass,
   long-poll wake, bad-token rejection (default-run, loopback only). mDNS test
   `#[ignore]`d (real multicast).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v2)

Internet relay / NAT traversal (v3), multi-hop propagation (A↔B↔C relays —
each pair syncs directly), selective partial-app sync, log compaction (its own
plan), syncing event kinds beyond the `kv.*` allowlist + CRDT + blob pass.

## Decisions to confirm

- **Transport TLS** — recommendation: plain HTTP on the LAN in v2 (pairing
  token gates access; self-signed cert UX is miserable). Alternatives: rustls
  with pinned self-signed certs exchanged at pairing; defer to the v3 relay.
- **Raw-TCP path retirement** — recommendation: deprecate in v2, delete in
  v2.1. Alternative: keep both transports indefinitely (two codepaths to test).
- **Allowlist scope** — recommendation: `kv.*` only. Alternatives: also
  `blob.stored`/`blob.removed` metadata (blob pass already covers bytes); an
  opt-in flag in `CapManifest` so caps self-declare replicable kinds.
- **mDNS crate** — recommendation: `mdns-sd`. Alternatives: `libmdns`
  (advertise-only), shelling to `dns-sd` on macOS (not portable).
