# 01 — Design decisions

Decide these before writing any code. Every later step follows from them.

## Is it a capability at all?

**New commands are new capabilities.** If Terrane needs a new verb, it gets a
crate under `rust/crates/terrane-cap-<name>/` implementing `Capability`. Never
reintroduce a central command/event enum or a central decide/fold match — the
registry is the only aggregation point.

It is **not** a new capability when:

- It's a new verb in an existing domain → add a command to that capability
  (e.g. `kv.storage.set` lives in `terrane-cap-kv`).
- It's host plumbing (rendering, transport, CLI flags, MCP tools) → it belongs
  in `rust/crates/terrane-host/` or a host under `host/`.
- It's app logic → it belongs in a JS bundle under `apps/`.

## Pick a shape

The shape determines what `decide` returns and which optional trait methods you
implement. Copy the archetype closest to yours.

| Shape | `decide` returns | Archetype | Use when |
|---|---|---|---|
| Pure | `Decision::Commit(events)` | `terrane-cap-kv`, `terrane-cap-app` | The outcome is fully determined by args + current state. |
| Effectful | `Decision::Effect(effect)` | `terrane-cap-net`, `terrane-cap-model`, `terrane-cap-replica` | You need the outside world once (HTTP, agent CLI, entropy); the *result* is recorded as an event. |
| Runtime | `Decision::Runtime(request)` | `terrane-cap-js-runtime`, `terrane-cap-wasm-runtime` | You execute app backends; guest writes become ordinary events. |
| Event-only | no commands | `terrane-cap-builder` | You only fold events that effects or other flows emit. |
| Resource-only | no commands, no events | `terrane-cap-build` | You expose a pure helper surface on `ctx.resource.<ns>` for app backends. |
| Projection over KV | `Decision::Commit` of `kv.*` events | `terrane-cap-relational-db` | You are a derived structure over reserved KV keys — no own state, no own events. |

Shapes compose: `app` is mostly pure but `app.import` is an effect; `kv` is
pure and also exposes resources.

## Naming

- **Namespace**: one short lowercase token, the crate suffix (`kv`, `net`,
  `relational_db`, `js-runtime`). It prefixes everything and must match — the
  registry rejects a command/event/query whose prefix isn't your namespace.
- **Commands**: `ns.verb`, nested where the domain has areas —
  `kv.set`, `kv.storage.set`, `auth.permission.request`, `harness.generate-app`.
- **Event kinds**: name the *fact*, past tense where it reads naturally —
  `kv.deleted`, `net.fetched`, `auth.granted`, `app.removed`,
  `replica.initialized`. Events are name-tagged on the wire:
  `EventRecord { kind: String, payload: Vec<u8> }` with a borsh-encoded payload
  struct owned by your crate.
- **Queries**: `ns.name`, read-only, exposed to other capabilities over the bus
  (`app.exists`, `replica.peer`).

## Where does state live?

| Option | When | Example |
|---|---|---|
| Own `State` slice | Default. A `#[derive(Debug, Clone, Default, PartialEq, Eq)]` struct of `BTreeMap`s. | `NetState`, `KvState`, `AuthState` |
| No state | Runtime and resource-only shapes; projections. | `js-runtime`, `build`, `relational_db` |
| Reserved-KV projection | Your data should live in the app's KV under `__terrane/…` reserved keys, rebuilt by folding ordinary `kv.*` events. | `relational_db` (rows/indexes), `auth` (projects grants alongside its slice) |

Use `BTreeMap`, never `HashMap` — deterministic iteration order is part of the
replay contract ([02-contract.md](02-contract.md)). Prefer an own slice unless
the data is genuinely app-scoped records (then project over KV).

## Keep it small

`terrane-cap-net` is a complete effectful capability in ~140 lines of
implementation. Start there in size and grow module-by-module only when a file
gets crowded ([03-skeleton-and-wiring.md](03-skeleton-and-wiring.md)).

Next: [02-contract.md](02-contract.md) — what the engine requires of you.
