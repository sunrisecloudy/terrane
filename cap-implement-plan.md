# Capability Interface Implementation Plan

This plan turns [`cap-design.md`](cap-design.md),
[`cap-design-review.md`](cap-design-review.md), and
[`cap-design-review-2.md`](cap-design-review-2.md) into an implementation
sequence.

The goal is not to build the final dynamic capability system in one pass. The
goal is to make the next architectural move that unlocks later cap crates while
preserving replay determinism and keeping the current engine understandable.

## Decision

Keep typed `State` for now.

Do not introduce opaque `CapStore<Vec<u8>>` yet. That belongs to the later
dynamic/WASM capability track. Today's engine gets its replay proof from typed
state equality, and replacing that with byte-store equality would make the
correctness story worse before it gets better.

The near-term cleanup is:

```text
typed State
+ generalized manifests
+ command/query contexts
+ read-only query bus
+ explicit cross-cap query APIs
```

That removes the coupling that blocks cap crates without paying the dynamic
runtime cost too early.

The order is settled:

```text
manifest -> contexts/query bus -> explicit subscriptions -> host service -> auth v1
```

Auth v1 follows the reshape. Phases 1-4 must not bake in assumptions that block
the later `manifest.resources ∩ granted` intersection; the host runtime's
namespace-filter step remains the chokepoint where that gate will live.

## Current Coupling To Remove

The current cross-cap read surface is small:

- `app.exists`
  - used by `kv`, `crdt`, `net`, `model`, and `harness`
  - today implemented as direct reads of `state.app.apps`
- `replica.peer`
  - used by `crdt`
  - today implemented as a direct read of `state.replica.peer`

Other cross-cap behavior is already event-shaped:

- `kv`, `crdt`, `net`, and `model` react to `app.removed`
- `harness` emits builder-related events through effect results

So the first decoupling step is a query bus, not a state-store rewrite.

## Target Near-Term Shape

### Capability Manifest

Replace the narrow `resource_api()` declaration with a broader manifest.

```rust
pub trait Capability {
    fn namespace(&self) -> &'static str;

    fn manifest(&self) -> CapManifest {
        CapManifest::default()
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision>;

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()>;

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {name}")))
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        None
    }
}
```

This is the near-term end-state sketch. Phase 1 introduces the manifest while
keeping the current `decide(&State, ...)` signature. Phase 2 changes `decide` to
receive `CommandCtx`.

`CapManifest` starts as a declaration surface, not a full schema engine:

```rust
pub struct CapManifest {
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub queries: Vec<QuerySpec>,
    pub resources: Vec<ResourceMethod>,
    pub subscriptions: Vec<EventPattern>,
}
```

The initial implementation can keep `ResourceMethod` as-is and move it under the
manifest. Command/event/query specs can be lightweight names and action classes.

### Command Context

Introduce a context even before auth needs `Subject`.

```rust
pub struct CommandCtx<'a> {
    pub state: &'a State,
    pub bus: &'a dyn CapBus,
}
```

This is the extension seam. Later auth can add `subject` here without touching
every capability again.

### Query Context

Queries are read-only.

```rust
pub struct QueryCtx<'a> {
    pub state: &'a State,
    pub bus: &'a dyn CapBus,
}
```

### Read-only Cap Bus

The bus reachable during `decide` must expose only queries.

```rust
pub trait CapBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue>;
}
```

It must not expose command dispatch or effects. This keeps `decide` pure.

### Query Values

Start with the values already needed by current caps.

```rust
pub enum QueryValue {
    Bool(bool),
    U64(Option<u64>),
}
```

Add `String`, `StringMap`, or `StringList` only when a real cap query needs
them. `ReadValue` remains separate for backend resource reads for now; we can
merge the two value enums later if they start to drift.

Do not switch commands from `args: &[String]` to structured `Value` yet. That is
larger host-contract churn and is not required for this step.

## Phase 1: Generalize Manifest

Purpose: make every cap's surface declarative and registry-visible.

Tasks:

1. Add `CapManifest`, `CommandSpec`, `EventSpec`, `QuerySpec`, and
   `EventPattern` to `terrane-core::cap`.
2. Move `resource_api()` into `manifest().resources`.
3. Keep a compatibility helper so runtime/doc generation can still ask for
   resources without broad churn.
4. Add manifests to every current cap:
   - `app`
   - `kv`
   - `crdt`
   - `net`
   - `model`
   - `replica`
   - `build`
   - `builder`
   - `harness`
   - `host` while it still exists as a cap
5. Teach the registry to validate duplicate command/query names and duplicate
   **owned/emitted** event names.
   - There must be one declaring owner per emitted event kind.
   - Subscriptions are not ownership and must not be treated as duplicate event
     declarations.
   - Example: `app` owns `app.removed`; `kv`, `crdt`, `net`, and `model` may
     subscribe to it later without declaring it as their own event.
6. Keep behavior unchanged.

Validation:

```sh
cd rust
cargo test --workspace --locked
cd ../host/cli && cargo test --locked
cd ../mcp && cargo test --locked
cd ../web && cargo test --locked
```

## Phase 2: Add Contexts And Query Bus

Purpose: remove direct cross-cap state reads.

Tasks:

1. Add `CommandCtx`, `QueryCtx`, `CapBus`, and `QueryValue`.
2. Change `Capability::decide` from:

   ```rust
   fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision>;
   ```

   to:

   ```rust
   fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision>;
   ```

3. Implement `CapBus` over an arbitrary `(&Registry, &State)` pair, not only
   over `Core`.
   - `Core::dispatch` is one `decide` call site.
   - `RunAccum::write` inside the QuickJS backend path is the second production
     `decide` call site.
   - `RunAccum::write` uses a per-run working `State` clone and a fresh per-run
     `Registry`; cap queries must resolve against that working pair so backend
     writes and later reads stay coherent inside a single run.
4. Add `Capability::query`.
5. Add queries:
   - `app.exists`
   - `replica.peer`
6. Convert direct reads:
   - `kv`: `state.app.apps.contains_key` -> `ctx.bus.query("app", "exists", ...)`
   - `crdt`: `state.app.apps.contains_key` -> `app.exists`
   - `crdt`: `state.replica.peer` -> `replica.peer`
   - `net`: `state.app.apps.contains_key` -> `app.exists`
   - `model`: `state.app.apps.contains_key` -> `app.exists`
   - `harness`: `state.app.apps.contains_key` -> `app.exists`
7. Keep typed own-state reads, for example `state.kv.data`, inside the owning
   capability only.

Known churn: Phase 2 touches `RunAccum::write`, and Phase 4 later moves that
runtime code into an explicit host service. That is acceptable; Phase 2 must
still update both `decide` call sites so behavior stays correct before Phase 4.

Validation:

- full Rust tests
- host tests
- macOS E2E
- a stale scan for direct cross-cap reads

Suggested scan patterns after conversion:

```sh
rg 'state\.app\.apps|state\.replica\.peer' rust/crates/terrane-core/src/cap
```

Only `app` should read `state.app.apps`; only `replica` should read
`state.replica.peer`, except transitional tests or core services.

## Phase 3: Make Subscriptions Explicit

Purpose: stop every `fold` from silently blind-matching unrelated event names.

Tasks:

1. Add subscription declarations to manifests:
   - `kv` subscribes to `app.removed`
   - `crdt` subscribes to `app.removed`
   - `net` subscribes to `app.removed`
   - `model` subscribes to `app.removed`
2. Keep broadcast fold behavior initially.
3. Add registry validation that every subscription references a declared event
   pattern.
4. Later, optimize by routing events only to declaring owner plus subscribers.

This phase should not change replay behavior.

## Phase 4: Host As Engine Service

Purpose: make explicit what the code already does.

Today `host.run` is special-cased because it needs `&mut Core` and executes app
backend JS. It is not a normal capability state machine.

Tasks:

1. Introduce a host/runtime service inside `terrane-core` or `terrane-host`.
2. Move resource installation and backend execution behind that service.
3. Make the service read registry manifests/resources instead of calling a fake
   `HostCapability`.
4. Keep emitted writes as normal capability events.
5. Remove or shrink `HostCapability` once routes/docs no longer need it.

Validation:

- all existing `host.run` tests
- macOS App Builder/BMI E2E
- replay tests proving JS is not rerun

## Phase 5: Auth v1 Seam

Purpose: support user-to-app confinement without changing replay.

Auth v1 should follow [`auth-design.md`](auth-design.md):

- app manifests request resources
- user grants a subset
- host installs `manifest.resources ∩ granted`
- generated apps start with zero grants

Tasks:

1. Add an `auth` capability for grant state:
   - `auth.granted`
   - `auth.revoked`
   - query: `auth.grantsForApp`
2. Add `Subject` only where it is load-bearing for v1: `host.run`.
3. Define a default local `Subject` path for API hosts.
   - CLI can use the implicit local owner.
   - Web and MCP invoke `host.run` through the public `InvokeRequest` contract,
     so they also need a default local subject until user/session identity
     exists.
4. Gate resource installation in the host runtime service at the existing
   namespace-filter chokepoint.
5. Do not run auth in `fold`.
6. Do not replay auth checks.

This is intentionally narrower than threading `Subject` through every dispatch
path. User-to-user auth can pay that bill later.

## Phase 6: Prepare Cap Crates

Purpose: make crate-per-cap possible once the interface is proven in one crate.

Tasks:

1. Extract interface-only items to either:
   - `terrane-core::cap_api`, or
   - a new `terrane-cap-api` crate
2. Move one simple cap first:
   - `kv` -> `terrane-cap-kv`
3. Move `net`, `model`, and `build`.
4. Move `replica`.
5. Move `crdt` only after the query bus and state ownership boundaries feel
   stable.
6. Move `builder` and `harness`.

`app` may stay close to core longer because it is foundational catalog state.

## Deferred Until Forced

Do not build these in the first implementation pass:

- opaque `CapStore<Vec<u8>>`
- structured `Command { payload: Value }`
- name-tagged `EffectRequest`
- `EventEnvelope { actor, cause, cap, cap_version }`
- dynamic native Rust plugin loading
- WASM or QuickJS app-installed capability loading

They are good eventual directions, but each changes persistence, host contracts,
or plugin safety. Build them only when there is a concrete consumer.

## Determinism Invariants

1. Only events mutate durable state.
2. Commands may read folded state but must not perform I/O.
3. The command context bus exposes read-only queries only.
4. Effects perform I/O once and return events.
5. Queries are read-only and never replayed.
6. Replay applies events only.
7. Auth gates commands, effects, and queries, never replay.
8. Capability event schemas remain backward-compatible.
9. Cross-cap reads go through query APIs, not direct state fields.
10. Cross-cap writes happen through recorded events and subscriptions.

## Success Criteria

The first milestone is complete when:

- every capability declares a manifest
- `resource_api()` is subsumed by manifest resources
- `Capability::decide` receives a context
- direct cross-cap reads are replaced by `CapBus::query`
- typed `State` still powers replay equality
- existing tests and macOS E2E pass
- no auth or dynamic store work has been prematurely added
