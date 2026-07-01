# 02 — The contract

A capability is one implementation of the `Capability` trait
(`rust/crates/terrane-cap-interface/src/capability.rs`):

```rust
pub trait Capability {
    fn namespace(&self) -> &'static str;
    fn manifest(&self) -> CapManifest { CapManifest::empty() }
    fn doc(&self, include_internal: bool) -> CapabilityDoc;
    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision>;
    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()>;
    fn describe(&self, record: &EventRecord) -> Option<String> { … }          // default: None
    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue>;      // default: unknown query
    fn read_resource(&self, ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue>; // default: unknown read
    fn resource_api(&self) -> Vec<ResourceMethod> { self.manifest().resources }
    fn grant_resource_specs(&self) -> Vec<GrantResourceSpec> { self.manifest().grant_resources }
    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput>;    // default: not a runtime
}
```

You always implement `namespace`, `manifest`, `doc`, `decide`, `fold`. The rest
have safe defaults — override only what your shape needs.

## The pipeline your code runs in

```
Request ─▶ Core::dispatch
             ├─ admit        (auth.* requires CommandAuthority::TrustedHost)
             ├─ decide       (your capability; pure routing + validation)
             │    ├─ Commit(events) ───────────────┐
             │    ├─ Effect(e)  ─▶ EffectRunner ───┤  runs ONCE at the edge, returns events
             │    └─ Runtime(r) ─▶ run_runtime ────┤  guest writes collected as events
             └─ commit(events)                     ▼
                  append to log ─▶ broadcast fold ─▶ State  (+ storage/projection sync)
```

`Core::dispatch` and `apply` live in `rust/crates/terrane-core/src/lib.rs`.

## Decide: pure routing and validation

- Route on the exact dotted name; unknown names return
  `Error::InvalidInput(format!("unknown command: {other}"))`.
- Parse args with the interface helpers
  (`rust/crates/terrane-cap-interface/src/helpers.rs`): `arg`, `join_tail`,
  `required_tail`, `non_empty`, `parse_usize_arg`, `ensure_app_exists`.
- Validate **eagerly and fully in decide** — fold assumes events are valid.
- Read your own slice via `state_ref::<YourState>(ctx.state, "ns")?`; ask other
  capabilities via `ctx.bus.query(...)` (read-only).
- Build events with `encode_event("ns.kind", &Payload { … })?`; payload structs
  derive `BorshSerialize, BorshDeserialize` and stay private to your crate.

## Fold: the deterministic half

`fold` is called for **every** recorded event from **every** capability
(broadcast) — that is how cross-capability cascades work with no coupling.

Rules:

- Match on `record.kind`; **unknown kinds fall through to `Ok(())`**. An error
  here halts replay of the whole log.
- A pure, deterministic function of `(state, event)` — no I/O, no clock, no
  randomness, no logging.
- Decode with `decode_event::<Payload>(record)?`; mutate via
  `state_mut::<YourState>(state, "ns")?`.
- Where a duplicated event would corrupt state, guard defensively — see
  `terrane-cap-replica` ("first identity wins" in its fold).

## Replay identity

Replaying the persisted log into a fresh `State` must reproduce the live state
exactly — `Core::replay_matches()` proves it and every engine test asserts it.
What this demands of you:

- Your state slice derives `Debug, Clone, Default, PartialEq` (add `Eq` when
  possible; the aggregate `State` itself is only `PartialEq` because the crdt
  slice can hold floats).
- `BTreeMap`/`BTreeSet` for collections — iteration order is observable.
- No wall-clock time, randomness, host paths, or environment reads in state or
  fold. Anything non-deterministic must arrive *inside an event* (see
  [05-effects-and-runtimes.md](05-effects-and-runtimes.md)).
- If you wrap a non-deterministic library, capture its output as the event.
  `terrane-cap-crdt` is the worked example: Loro embeds peer IDs, so `decide`
  applies the op to a fork and records the resulting *export bytes*; `fold`
  imports those exact bytes.

## Errors

Use the typed enum from `rust/crates/terrane-cap-interface/src/abi.rs` — never
define your own, never panic on real paths (no `unwrap`/`expect`/`panic!`):

| Variant | Use for |
|---|---|
| `InvalidInput(msg)` | Bad args, unknown command/query names, validation failures |
| `AppNotFound` / `AppExists` / `KeyNotFound` | The shared domain errors — reuse them |
| `Storage(msg)` | Serialization and persistence failures |
| `Runtime(msg)` | Execution/logic failures at run time |

`describe` is the one place decoding failures are swallowed: use
`decode_event(record).ok()?` and return `None` for foreign or corrupt payloads.

## What the registry validates

`Registry::validate()` (`rust/crates/terrane-core/src/lib.rs`) runs when the
default registry is built and fails loudly on:

- a command/event/query whose prefix isn't the declaring namespace;
- two capabilities claiming the same command, event kind, or query;
- a subscription to an event kind nobody declares;
- `resources` without `grant_resources` (or the reverse), or grant verbs that
  don't cover the declared method kinds.

Declare everything you own — and only what you own — in `manifest()`.

Next: [03-skeleton-and-wiring.md](03-skeleton-and-wiring.md) — the crate and its three wiring points.
