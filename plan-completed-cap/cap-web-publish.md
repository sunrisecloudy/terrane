# Capability: `web-publish` — public URLs for apps (Premium)

Small capability crate `rust/crates/terrane-cap-web-publish/` (namespace
`web-publish`) for the recorded facts, plus substantial host + Premium-relay
work. Gives an app a public internet URL that anonymous visitors can open —
the Val-Town/Glitch "it's live" moment — without the home host accepting any
inbound connection.

## Locked decision (user, 2026-07-05)

**Premium-gated.** Public serving goes through the Premium relay and is
available to logged-in/paid users only. The local host never opens an inbound
port; it dials **out** to the relay and keeps a tunnel alive. Free/local users
keep LAN serving (existing web host) — nothing public.

## How it works

```
visitor ──HTTPS──▶ relay (<slug>.terrane.app, TLS, rate limits, abuse)
                     │  outbound wss tunnel (host-initiated, token-authed)
                     ▼
              home host ──▶ serves app UI / allowlisted verbs
```

- The relay is a thin, stateless-ish proxy in `../terrane-premium`: TLS
  termination, slug→tunnel routing, per-slug rate limiting, abuse controls.
  App data never rests on the relay (privacy stance: transit only).
- Offline homes = offline site in v1 (relay serves a "temporarily offline"
  page). Static snapshot pinning at the relay is a Decision to confirm.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `web-publish.enable` | `{app, mode: static\|interactive, slug?}` → recorded `web-publish.enabled {app, mode, slug}`; slug allocation is an edge effect against the relay (recorded result) |
| Command | `web-publish.disable` | recorded `web-publish.disabled {app}`; edge tears the route down |
| Command | `web-publish.domain.set` | custom domain (paid tier) → recorded; relay handles ACME |
| Query | `web-publish.status` | folded: enabled apps, mode, slug/domain; live tunnel health is a transient host read |
| Event | `web-publish.enabled/disabled/domain.set` | facts only — visitor traffic is **never** recorded (transient by definition; anonymous request logs would bloat the log and leak visitor privacy). Optional aggregate counters are a Decision to confirm. |

## Modes

- **`static`** (default): the relay path serves only the app's UI bundle and
  read-only state the UI was built with. No verb invocation. Safe by default.
- **`interactive`**: visitors may invoke verbs listed in the manifest's
  `publicVerbs` allowlist, executed as principal `anonymous` with a dedicated
  auth grant class — the app opts in per verb, the platform enforces at the
  host boundary, and per-IP rate limits apply at the relay. `common.receive`
  is NOT public unless listed (being a delivery target for the owner ≠ being
  writable by the internet).

## Security & privacy

- Tunnel auth: relay tokens minted against the Premium account (Google login
  exists) and stored in the host keychain per
  [cap-oauth-connections.md](cap-oauth-connections.md); never in events.
- Anonymous principal can never escalate: no permission prompts reach the
  owner from visitor traffic — missing grant = 403, full stop.
- Slug enumeration resistance (random default slugs), relay-side abuse
  (request caps, body-size caps mirroring [cap-net-v2.md](cap-net-v2.md)
  limits), and a one-click `disable` kill switch.

## Limits

Interactive requests ≤ 1 MiB body; per-slug rate limits set relay-side;
`publicVerbs` ≤ 16; one slug per app, domains per paid tier.

## Implementation plan

1. **Crate:** `terrane-cap-web-publish` — commands/events/fold/doc (facts only).
2. **Host tunnel client:** outbound wss with reconnect/backoff in
   `terrane-host`, request bridging into the existing web-host serving path;
   anonymous principal enforcement + `publicVerbs` check.
3. **Relay service:** in `../terrane-premium` (TS) — tunnel registry, TLS,
   slugs, rate limits, offline page, ACME for custom domains.
4. **Manifest:** `publicVerbs` field + validation.
5. **Shell UI:** enable/disable + slug display in web/mac shells; Premium
   login gating.
6. **Tests:** engine (grant/mode facts, replay); e2e with a loopback fake
   relay (tunnel bridging, anonymous 403s, allowlisted verb 200s) — real relay
   tests live in terrane-premium.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Static snapshot serving while the home is offline (Decision to confirm for
v2), visitor accounts/sessions, server-side rendering, multi-region relays,
vanity slugs marketplace.

## Decisions to confirm

- **Offline behavior** — recommend v1 "offline page", v2 optional
  relay-pinned static snapshot (privacy trade-off: bundle rests on relay) —
  alternative: always-offline-page only.
- **Aggregate visitor counters** — recommend relay-side daily counts surfaced
  as a transient query (nothing in the log) — alternative: recorded daily
  rollup events for history.
- **Free-tier teaser** — recommend none (clean Premium gate) — alternative:
  time-limited preview URLs.
