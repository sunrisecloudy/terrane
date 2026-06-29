# Terrane Capability Design

This note captures the target capability interface for Terrane after the Rust
workspace cleanup. The goal is to let capabilities move into separate crates and
eventually support app-installed capabilities without breaking deterministic
replay.

## Core Principle

Terrane capabilities are CQRS-style bounded contexts:

- commands express intent
- events are recorded facts
- apply/replay mutates state from facts only
- queries read state only
- effects perform external work once and return facts

The deterministic contract is:

```text
CommandEnvelope(subject, command)
  -> authorize against current folded policy state
  -> command handler reads cap stores
  -> returns events or effect requests

EffectRequest
  -> host performs non-deterministic work once
  -> records result as events

EventEnvelope(actor, cause, event)
  -> append to log
  -> apply to capability stores

Replay
  -> read EventEnvelope stream
  -> apply events to capability stores
  -> never rerun commands
  -> never rerun auth
  -> never rerun effects
  -> never rerun queries
```

Only events mutate durable state. Anything non-deterministic must be converted
into recorded event data before it affects durable state.

## Current Capabilities

| Capability | State | Commands | Events | Effects | Resource API | Cross-cap behavior |
| --- | --- | --- | --- | --- | --- | --- |
| `app` | yes | yes | yes | no | no | foundation for app existence/catalog |
| `kv` | yes | yes | yes | no | yes | reads `app`, reacts to `app.removed` |
| `crdt` | yes | yes | yes | no direct effect | yes | reads `app` and `replica`, reacts to `app.removed` |
| `net` | yes | yes | yes | yes | no | reads `app`, reacts to `app.removed` |
| `model` | yes | yes | yes | yes | no | reads `app`, reacts to `app.removed` |
| `replica` | yes | yes | yes | yes | no | read by `crdt` |
| `build` | no | no real commands | no | no | yes | resource-only compiler helper |
| `builder` | yes | no direct commands | yes | no | no | folded by harness-generated events |
| `harness` | yes | yes | yes | yes | no | reads `app`, uses builder validation/events |
| `host` | no | special engine path | no own events | no | executes resources | depends on registry/runtime |

This shows that a capability interface must not assume every capability has
state, commands, events, resources, and effects. Those are optional surfaces.

## Do Not Centralize State

The new design should not replace today's typed `State` with a central
`CoreState { caps: BTreeMap<CapId, CapState> }` model. That still makes core the
owner of capability state layout.

Instead:

- core owns the log, registry, store, authz service, and effect runner
- each capability owns its own state schema inside a scoped store
- cross-cap reads happen through queries, not direct field access

```rust
pub trait CapStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&mut self, key: &str, value: Vec<u8>) -> Result<()>;
    fn delete(&mut self, key: &str) -> Result<()>;
    fn scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;
}
```

The store passed to a capability is already scoped to that capability. A `kv`
cap should not need to pass `"kv"` into its own store operations.

## Generalized Capability Interface

The interface should be general enough for future capabilities, but it should
keep CQRS phases explicit. A single generic `handle()` path is useful as a low
level plugin ABI later, but core Rust caps should keep command/apply/query
separate so determinism is obvious.

```rust
pub trait Capability {
    fn id(&self) -> CapId;
    fn version(&self) -> CapVersion;

    fn manifest(&self) -> CapManifest;

    fn init(&self, ctx: InitCtx) -> Result<()> {
        Ok(())
    }

    fn command(&self, ctx: CommandCtx, command: Command) -> Result<CommandOutcome> {
        Err(Error::UnknownCommand(command.name))
    }

    fn apply(&self, ctx: ApplyCtx, event: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn query(&self, ctx: QueryCtx, query: Query) -> Result<QueryResult> {
        Err(Error::UnknownQuery(query.name))
    }

    fn describe(&self, event: &EventRecord) -> Option<String> {
        None
    }
}
```

The manifest declares the public surface and routing metadata:

```rust
pub struct CapManifest {
    pub id: CapId,
    pub version: CapVersion,
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub queries: Vec<QuerySpec>,
    pub resources: Vec<ResourceSpec>,
    pub effects: Vec<EffectSpec>,
    pub subscriptions: Vec<EventPattern>,
}
```

Capabilities are message handlers with private state and declared surfaces, but
the core interface names the important CQRS phases.

## Contexts

Each phase gets only the access it needs.

```rust
pub struct CommandCtx<'a> {
    pub subject: &'a Subject,
    pub store: &'a dyn CapStore,
    pub bus: &'a dyn CapBus,
}

pub struct ApplyCtx<'a> {
    pub store: &'a mut dyn CapStore,
    pub bus: &'a dyn CapBus,
}

pub struct QueryCtx<'a> {
    pub subject: &'a Subject,
    pub store: &'a dyn CapStore,
    pub bus: &'a dyn CapBus,
}
```

Commands and queries carry a subject because they are authorized. Event apply
does not authorize; replay must apply recorded facts directly.

Cross-cap reads go through `CapBus`:

```rust
pub trait CapBus {
    fn query(&self, cap: &str, name: &str, payload: Value) -> Result<Value>;
}
```

Examples:

```text
ctx.query("app", "exists", { "app": "todo" })
ctx.query("replica", "peer", {})
ctx.query("kv", "all", { "app": "todo" })
```

## Commands, Events, Queries, Effects

```rust
pub struct Command {
    pub name: String,
    pub payload: Value,
}

pub struct Query {
    pub name: String,
    pub payload: Value,
}

pub struct EventRecord {
    pub kind: String,
    pub schema_version: u32,
    pub payload: Vec<u8>,
}

pub struct EffectRequest {
    pub kind: String,
    pub schema_version: u32,
    pub payload: Vec<u8>,
}

pub enum CommandOutcome {
    Events(Vec<EventRecord>),
    Effects(Vec<EffectRequest>),
    EventsAndEffects {
        events: Vec<EventRecord>,
        effects: Vec<EffectRequest>,
    },
    None,
}
```

Events and effects are opaque to core. Their schema and encoding are owned by
the capability that declares them.

## Auth

Auth is outside replay and inside command/query/effect gates.

```text
auth is checked before intent becomes fact
replay applies facts without auth
```

Core should enforce auth centrally, using permissions declared in capability
manifests:

```rust
pub struct CommandSpec {
    pub name: &'static str,
    pub permission: Permission,
}

pub struct QuerySpec {
    pub name: &'static str,
    pub permission: Permission,
}

pub struct EffectSpec {
    pub kind: &'static str,
    pub permission: Permission,
}
```

An `auth` capability can own policy state:

- users and local identities
- app grants
- sessions and tokens
- peer trust
- role bindings

But enforcement should be a core service so individual capabilities cannot
forget to check it.

Policy changes are events:

```text
auth.grant.created
auth.grant.revoked
auth.role.assigned
```

Replay applies those policy events like any other events. It does not re-run
authorization against old commands.

## Event Envelopes and Audit

Event metadata should be separated from capability payloads:

```rust
pub struct EventEnvelope {
    pub id: EventId,
    pub actor: SubjectRef,
    pub cause: Option<CommandId>,
    pub cap: CapId,
    pub cap_version: CapVersion,
    pub record: EventRecord,
}
```

Replay uses `cap`, `cap_version`, and `record`. Actor/cause are audit metadata
and should not affect application of the event.

## Dynamic Capabilities

Start with compile-time Rust cap crates:

```text
terrane-core
terrane-cap-api
terrane-cap-app
terrane-cap-kv
terrane-cap-crdt
terrane-cap-net
terrane-cap-model
terrane-cap-build
terrane-cap-builder
terrane-cap-harness
terrane-cap-replica
```

Do not start by loading arbitrary native Rust dynamic libraries from apps. Rust
dylib/plugin ABI stability and trust boundaries are the wrong first step.

For app-installed capabilities later, prefer:

- WASM capability plugins
- QuickJS capability modules with strict sandboxing
- declarative manifests that bind to host-provided primitives

The same CQRS interface can be wrapped into a lower-level message ABI later:

```rust
pub struct CapMessage {
    pub kind: MessageKind,
    pub name: String,
    pub payload: Value,
}

pub enum MessageKind {
    Command,
    Event,
    Query,
    ResourceCall,
    System,
}
```

That wrapper should preserve the same phase rules.

## Applying This To Current Capabilities

### app

Owns app catalog state.

Commands:

- `app.add`
- `app.remove`

Events:

- `app.added`
- `app.removed`

Queries:

- `app.exists`
- `app.get`
- `app.list`

Other caps use `app.exists` instead of reading `state.app.apps`.

### kv

Owns app-scoped string key/value state.

Commands:

- `kv.set`
- `kv.rm`

Events:

- `kv.set`
- `kv.deleted`

Queries/resources:

- `kv.get`
- `kv.all`

Subscriptions:

- `app.removed` to delete all keys for that app

### crdt

Owns Loro documents per app.

Commands:

- `crdt.mapSet`
- `crdt.mapDel`
- `crdt.listPush`
- `crdt.listInsert`
- `crdt.listDel`
- `crdt.textInsert`
- `crdt.textDel`
- `crdt.merge`

Events:

- `crdt.update`

Queries/resources:

- `crdt.mapGet`
- `crdt.mapAll`
- `crdt.listAll`
- `crdt.textGet`
- sync/export queries

Cross-cap queries:

- `app.exists`
- `replica.peer`

Subscriptions:

- `app.removed`

### net

Owns recorded fetch responses.

Commands:

- `net.fetch`

Effects:

- `net.httpGet`

Events:

- `net.fetched`

Subscriptions:

- `app.removed`

### model

Owns recorded model turns.

Commands:

- `model.ask`

Effects:

- `model.call`

Events:

- `model.responded`

Subscriptions:

- `app.removed`

### replica

Owns local replica identity.

Commands:

- `replica.init`

Effects:

- `replica.newId`

Events:

- `replica.initialized`

Queries:

- `replica.peer`

### build

Resource/query-only compiler helper.

Queries/resources:

- `build.compileTs`

No durable state and no events.

### builder

Owns app generation draft state.

Events:

- `builder.requested`
- `builder.generated`
- `builder.failed`

Queries:

- `builder.getDraft`
- `builder.listDrafts`

No direct commands in the target shape unless we intentionally make builder a
user-facing command surface. Harness can emit builder events after effects.

### harness

Owns harness JS-run state and emits app-generation effects.

Commands:

- `harness.generateApp`
- `harness.runJs`

Effects:

- `harness.generateApp`
- `harness.runJs`

Events:

- `harness.js.requested`
- `harness.js.generated`
- `harness.js.completed`
- `harness.js.failed`

Cross-cap queries:

- `app.exists`

### host

`host` should probably become an engine service, not a normal capability. It
executes app backends and resource APIs by using the registry. Its writes should
still become normal capability events, but `host.run` itself should not own
durable state.

## Migration Plan

1. Extract `terrane-cap-api` or an internal `cap_api` module from
   `terrane-core` with `Capability`, contexts, manifests, command/event/query
   types, effect requests, permissions, and `CapStore`.
2. Introduce scoped `CapStore` while keeping today's typed `State` as a
   compatibility layer.
3. Add query APIs for current cross-cap reads: `app.exists`, `replica.peer`,
   `kv.all`, builder draft reads.
4. Convert `kv` first because it has simple state, commands, resources, and one
   subscription.
5. Convert `net`, `model`, and `build`.
6. Convert `replica`.
7. Convert `crdt` after query/store semantics are proven.
8. Convert `builder` and `harness`.
9. Move converted capabilities into `terrane-cap-*` crates.
10. Rework `host` into an engine service over registry resource specs.
11. Add capability versioning and migration tests.
12. Only after built-in Rust crates are stable, design WASM/QuickJS app-installed
    capability loading.

## Non-negotiable Invariants

1. Only events mutate durable state.
2. Commands may read folded state but must not perform I/O.
3. Effects perform I/O once and return events.
4. Queries are read-only and never replayed.
5. Replay applies events only.
6. Auth gates commands, effects, and queries, never replay.
7. Capability event schemas are versioned and backward-compatible.
8. Cross-cap reads go through query APIs, not direct state fields.
9. Capability stores are private to the owning capability.
10. Dynamic capability code must be versioned, sandboxed, and replay-compatible.
