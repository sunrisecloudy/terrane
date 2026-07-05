# Capability: `push` — notify devices whose host is not running

New crate `rust/crates/terrane-cap-push/`, namespace `push`, registered in
`default_registry`. The honest problem: local-first has **no always-on
server**, so "push" in the APNs sense is impossible without infrastructure we
have deliberately not built. What we *can* do today: every replica the user
pairs (see `cap-sync-v2.md`) eventually receives the app's events, and any
**running** host can raise a native notification via the existing `native`
capability (`native.notification.show` /
`ctx.resource.native.notificationShow`). Push v1 is exactly that composition.

## Locked decision

**v1 is "local push": subscriptions are synced facts, delivery is a local edge
effect on whichever of the user's hosts is running.** `push.subscribe` records
a durable subscription (a `kv.*`-class replicable fact carried by sync v2's
event channel). Every running host folds the subscriptions and watches events
as they arrive — locally dispatched or ingested via `sync.apply` — and when
one matches, delivers a native notification through the `native` cap's
existing op queue. No third-party infra, no tokens leaving the LAN, no new
trust. A device whose host is not running gets the notification **when its
host next starts and catches up** (with a staleness cutoff, below) — that
limitation is stated plainly in `doc.rs` and in this spec, not papered over.
A true wake-a-sleeping-phone push needs a relay and is a v2 decision, not a
v1 default.

## Capability surface

### Commands

| Command | Args | Decision |
| --- | --- | --- |
| `push.subscribe` | `app, event_pattern, template, sub_id?` | Validate pattern (exact kind or `ns.*`) + template; emit `push.subscribed`. |
| `push.unsubscribe` | `app, sub_id` | Pure: emit `push.unsubscribed`. |
| `push.record-delivery` | `app, sub_id, event_seq, status(delivered\|failed), detail?` | Pure: emit `push.delivered` or `push.failed` — dispatched by the edge after the attempt. |

### Events

| Kind | Payload | Fold |
| --- | --- | --- |
| `push.subscribed` | `{app, sub_id, event_pattern, template}` | upsert `app → sub_id → sub` — **synced** (in sync v2's allowlist) |
| `push.unsubscribed` | `{app, sub_id}` | drop sub — synced |
| `push.delivered` | `{app, sub_id, event_seq}` | append to bounded delivery history — **replica-local, never synced** (this device notified; other devices deliver independently) |
| `push.failed` | `{app, sub_id, event_seq, detail}` | same, replica-local |
| (reacts) `app.removed` | — | drop subs + history for the app |

Template: a string with `{field}` placeholders resolved from the matched
event's `describe()` line and payload fields (title/body split on first `|`).
Deterministic string-in/string-out; rendering is pure and tested.

### Resource methods (JS: `ctx.resource.push`)

| Method | Semantics |
| --- | --- |
| `subscribe(pattern, template)` | routes to `push.subscribe`; returns `sub_id` |
| `unsubscribe(subId)` | routes to `push.unsubscribe` |
| `list()` | JSON of the app's subscriptions (pure state read) |

Grant resource: `push` namespace-v1 with `subscribe` — the permission prompt
says "show system notifications when this app's data changes."

## Delivery semantics (edge, per running host)

- Watcher in `terrane-host`: after each commit/ingest, match new records
  against folded subscriptions; on match, enqueue
  `native.notification.show` with the rendered template, then dispatch
  `push.record-delivery` with the outcome. Attempts are effects; only their
  **outcomes** are recorded — replay never re-notifies (fold of
  `push.delivered/failed` is bookkeeping only).
- **Staleness cutoff:** on startup/catch-up, events older than 24 h (default,
  home-configurable) are not notified — a week-offline laptop must not open to
  400 stale banners. Skipped matches record nothing.
- **Dedup per replica:** at most one notification per `(sub_id, event)` per
  replica, keyed by the delivery history. Different replicas each deliver once
  — that is per-device notification behavior, and correct.
- Rate: ≤ 1 notification per subscription per 10 s, coalescing into an
  "N changes in <app>" summary body; ≤ 32 subscriptions per app.

## Replay & sync

Subscriptions ride sync v2's event channel, so subscribing on the desktop arms
every paired device. Delivery events stay local: replaying a home's log
rebuilds its own delivery history exactly and triggers zero notifications
(delivery is an effect of *new* matches at the edge, never of folding).

## Implementation plan

1. **Crate `terrane-cap-push`:** state, decide/fold/describe, pattern +
   template validation and pure rendering, `doc.rs` (the "only running hosts
   deliver" paragraph verbatim); register in `default_registry`; add
   `push.subscribed/unsubscribed` to sync v2's replication allowlist.
2. **Watcher** (`terrane-host/src/push_watch.rs`): post-commit/post-ingest
   match → `native.notification.show` dispatch → `push.record-delivery`;
   staleness cutoff + dedup + coalescing.
3. **Hosts:** web host surfaces deliveries for its platform (Web Notifications
   from the shell when the tab is open); macOS host relies on its existing
   native-op handling — no new UI.
4. **App surface:** `APP_API.md` for `ctx.resource.push`; CLI
   `terrane push ls|rm <app>`.
5. **Tests:** engine tests `terrane-core/tests/cap/push.rs`
   (subscribe/unsubscribe folds, template rendering, replay identity,
   replay-delivers-nothing invariant, app.removed); e2e
   `terrane-host/tests/cap/push.rs` — subscribe, commit a matching event,
   assert one `native.notification.show` op + one `push.delivered`; sync a
   matching event from a second temp home and assert delivery there;
   staleness cutoff and dedup (default-run, no real notification UI —
   asserts on the native op queue).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Waking devices that aren't running (see below), APNs/FCM integration, rich
notification actions/deep links (a follow-up on the native cap), per-event
read/ack state across devices, notifying non-owner peers (subscriptions arm
the subscribing user's own replicas; shared-app peers subscribe themselves).

## Decisions to confirm

- **v2 remote relay** — recommendation: defer; when needed, an *optional*
  relay (user-provided or Premium-hosted) that holds APNs/FCM tokens and
  receives only `{app_id, sub_id}` wake hints — never event payloads — so the
  relay learns "something changed in app X," not what. That metadata leak is
  the privacy price and must be stated in the consent prompt. Alternatives:
  payload-carrying relay with E2E encryption (more moving parts, real pushes
  can show content); no relay ever (local push only, simplest honest story).
- **Staleness cutoff default** — recommendation: 24 h. Alternatives: never
  notify on catch-up (live matches only); unlimited with a summary banner.
- **Web-host delivery channel** — recommendation: shell-side Web Notification
  when a tab is open, nothing otherwise. Alternative: service-worker Web Push
  (requires a push service = the relay decision by another name).
