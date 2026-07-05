# Primitive: item — every thing in every app is addressable

A convention plus validation, not a crate: **`terrane://app/<appId>/item/<itemId>`**
names any item in any app, resolved live through the app's own `common.get`.
The platform stores nothing — item existence is always the app's answer, so
the address can never drift from app truth (the registry alternative was
rejected for exactly that drift).

## Locked decision (user, 2026-07-05)

**The `items` interface is required on every app**, joining `common.receive`
in the mandatory set: every bundle must implement `common.list` and
`common.get`. An app with no natural items (a calculator) satisfies it with
the scaffold default — `common.list` returns `[]`, `common.get` returns a
typed not-found — an empty item space is a valid item space. This amends
[cap-interop.md](cap-interop.md)'s required-verb decision: **required =
`common.receive` + `common.list` + `common.get`**; `search`/`export`/`glance`
stay optional.

## Contract

- `common.list(filterJson?)` → JSON array of `{id, title, kind}`; ids are
  app-chosen opaque strings, stable for the item's lifetime.
- `common.get(id)` → the item as JSON (`{id, title, kind, …}`) or the typed
  not-found reply — never a crash.
- URI grammar: `terrane://app/<appId>/item/<itemId>` (itemId
  percent-encoded). Producers and consumers of item URIs:

| Consumer | Behavior |
| --- | --- |
| [cap-deep-links.md](cap-deep-links.md) | `open` an item URI → shell opens the app focused on the item (`common.receive("link", {item})` tells the app which); `send` unchanged |
| interop picker ([cap-interop.md](cap-interop.md)) | `interop.pick("items")` gains item mode: user picks app **and item**; returns the URI |
| [cap-automation.md](cap-automation.md) payloads, search results, [cap-history.md](cap-history.md) panel | reference items by URI — one address format everywhere |
| cross-app references | any app can store another app's item URI and resolve it later via `interop.call(target, "common.get", id)` under its interop grant |

Resolution authorization = interop's existing grant model; an item URI is a
name, never a bearer token.

## Implementation plan

1. **Contract:** `APP_API.md` — items interface, URI grammar, stability rules,
   the empty-space default.
2. **Scaffold:** default `common.list`/`common.get` over the `items/` kv
   prefix (returns `[]` when unused); generated apps get real implementations.
3. **Validation:** builder validate + `app.import` extend the
   [cap-interop.md](cap-interop.md) probe: `common.list` must return a JSON
   array; `common.get` on a listed id must return JSON, on a bogus id the
   typed not-found. Repo apps patched in the same slice as the
   `common.receive` rollout (one migration, not two).
4. **URI plumbing:** parser/formatter helper in `terrane-cap-interface`
   (single implementation); deep-links route + shell open-at-item; picker
   item mode.
5. **Tests:** validation acceptance/rejection fixtures, URI round-trip,
   e2e: app A stores app B's item URI, resolves it via interop, deep-link
   opens B at the item.

Gate: `cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Global item index (query/search caps materialize their own views), item-level
sharing across users (rides crdt/document v2 as noted in
[cap-share-invite.md](cap-share-invite.md)), item change notifications
(automation on the owning app's events covers it), dangling-link detection
(resolution answers honestly).

## Decisions to confirm

- **Focused-open delivery shape** — recommend `common.receive("link",
  {item})` so apps need no new verb — alternative: a dedicated
  `common.open(id)` verb (cleaner intent, one more required verb).
- **Item ids stability rule** — recommend "stable for the item's lifetime,
  never reused" as documentation-level contract — alternative: validation
  probes for reuse (unenforceable in general; skip).
