# 04 — Cross-capability interaction

Capabilities never call each other directly and never import each other's
internals. Three sanctioned channels exist; everything else is coupling.

## 1. Bus queries (read-only, at decide time)

Ask another capability a question through `ctx.bus.query(cap, name, args)`
(`CapBus`, `rust/crates/terrane-cap-interface/src/runtime.rs`). Answers are
`QueryValue::Bool | U64` — extend the enum deliberately, not casually.

- The owning capability declares the query in its manifest (`QuerySpec`) and
  implements `query()` — see `replica.peer` in `terrane-cap-replica`.
- Common queries get a typed wrapper in the interface `helpers` so callers
  don't hand-roll matches: `app_exists`, `ensure_app_exists`, `replica_peer`.
  Add a wrapper when a second capability needs your query.
- Queries cannot mutate: they see `&dyn StateStore`. Don't create query cycles.

## 2. Broadcast-fold reactions (event subscriptions)

Every recorded event is offered to every capability's `fold`. To *react* to an
event you don't own, declare it in your manifest —

```rust
subscriptions: vec![EventPattern { kind: "app.removed" }],
```

— and match its kind in `fold`. The registry rejects subscriptions to event
kinds nobody declares. Decode foreign payloads only through helpers exported
for that purpose (`decode_app_removed` in the interface `helpers`), never by
copying the payload struct.

**If you hold per-app state, reacting to `app.removed` is effectively
mandatory.** `kv`, `net`, `model`, `crdt`, and `auth` all drop that app's slice
on it. A past review (006) found `auth` skipping this: revoked apps kept their
grants, and a reinstalled app silently inherited old permissions. The engine
test for this cascade is `kv_records_and_cascades_via_broadcast_fold` in
`rust/crates/terrane-core/tests/cap/kv.rs`.

## 3. Reserved-KV projections

When your data is app-scoped records rather than engine state, don't build a
parallel store — project into the app's KV namespace under reserved keys and
let `kv` own persistence and replay.

- Reserved keys start with `__terrane/` (`terrane_cap_kv::RESERVED_PREFIX`).
  The public resource surface rejects them, so apps can't read or forge yours.
  (The mirror-image primitive is `kv`'s **public** bucket — host-written,
  app-readable — which i18n uses for `i18n/<code>/<domain>.<key>` translations;
  see [09-docs-and-done.md](09-docs-and-done.md).)
- Namespace your own area and version it:
  `relational_db` uses `__terrane/rdb/v1/…` (`RDB_PREFIX`), `auth` projects
  under `__terrane/auth/v1` (`AUTH_PROJECTION_KEY_PREFIX`).
- Emit and read through `terrane-cap-kv`'s exported helpers — `set_event`,
  `delete_event`, `get_value`, `scan_prefix`, `scan_range`,
  `delete_prefix_events` — so the events on the log are ordinary `kv.*` events.

`terrane-cap-relational-db` is the pure archetype: **zero own events, zero own
state, an empty fold**. Its `decide` validates rows against the stored schema
and returns `kv.set`/`kv.deleted` events; replay rebuilds everything because
`kv` folds them. If you find yourself designing "a store, but for X", start
here.

## Which channel, when

| You need… | Channel |
|---|---|
| A fact from another cap while validating a command | Bus query |
| To keep your state consistent when another cap's facts change | Subscription + fold |
| To store app-scoped records durably | Reserved-KV projection |
| To call another cap's function | You don't. Reshape into one of the above. |

Next: [05-effects-and-runtimes.md](05-effects-and-runtimes.md) — the non-pure shapes.
