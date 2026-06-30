# Storage Target Plan

This document separates the storage target from auth correctness.

Auth correctness depends on:

```text
auth.* events
AuthState
deterministic replay
reserved internal auth projection
```

It should not depend on finishing a broader storage-engine migration first.

## Current Grounding

Today, Terrane storage is split:

```text
event log
  append-only borsh log file

core state
  folded in memory from the log

kv state
  folded KvState in memory
  optional external SQLite/RocksDB projection for app KV
```

That split is acceptable for starting the auth capability.

The storage target below is a product/runtime target, not a prerequisite for the
first auth gate.

## Target Direction

Default future local storage should use the same physical DB backend family for:

```text
event log
public app KV projection
reserved internal auth projection
auth audit/query indexes
other capability projections
```

This does not mean every capability shares semantics. It means the same storage
backend family hosts multiple isolated logical stores.

Ownership remains:

```text
auth owns auth.* events, AuthState, and auth projections
kv owns kv.* events, KvState, and public app KV projections
app owns app.* events and AppState
other capabilities own their own events and projections
```

## Auth Storage Requirement

Auth has a stricter requirement than the global event-log target:

```text
Auth MUST project to reserved internal storage using the same physical DB backend
family as public KV projections.
```

Meaning:

```text
public app KV projection
  stored with selected local DB backend family
  visible only through ctx.resource.kv for the owning app

reserved auth projection
  stored with the same backend family
  not visible through ctx.resource.kv
  not visible through public KV scans
  accessed only by auth/admin/runtime trusted paths
```

Auth still owns the policy meaning. It must not emit `kv.set` events to model
grants.

## Event Log Storage Target

The event log should eventually default to the same physical DB backend family
as projections.

Target picture:

```text
same physical backend by default
  event_log table / column family
  kv_public projection table / column family
  auth_reserved projection table / column family
  auth_audit projection table / column family
```

This target can change later. The important property is that event facts remain
append-only and replayable even if the storage backend changes.

## Non-Goals For Auth V1

Auth v1 should not require:

- migrating the append-only borsh event log into SQLite/RocksDB;
- designing the full storage abstraction for all capabilities;
- changing KV public API semantics;
- exposing auth records through `ctx.resource.kv`;
- making event-log storage depend on auth.

Auth v1 can start with:

```text
auth.* events in the existing event log
AuthState folded in memory
reserved internal projection using the selected KV backend family
```

## Logical Store Separation

If one physical DB file/backend is used, it must still separate logical stores.

Example logical stores:

```text
event_log
projection/kv/public/<app>
projection/auth/reserved
projection/auth/audit
projection/relational_db/<app>
```

Each logical store needs its own access rules.

Generated apps can access only the runtime resources granted to them. They never
receive generic access to the physical DB.

## Replay Rule

Storage projections are derived data.

Replay source of truth:

```text
event log -> fold -> state -> rebuild projections
```

If a projection is missing, corrupted, or uses an old schema, Terrane should be
able to rebuild it from events or perform a recorded migration.

Do not make projection contents the authoritative policy source.

## Determinism

The storage backend must not change replay semantics.

Rules:

- event ordering is stable;
- projection rebuild is deterministic;
- auth gates read folded `AuthState`, not live wall-clock or external DB state;
- storage compaction does not remove facts needed for replay;
- migrations that change facts must be recorded as events.

## Storage Backend Selection

The storage backend family should be explicit in local configuration.

Example planning shape:

```text
storage.default_backend = memory | sqlite | rocksdb
storage.event_log_backend = default | borsh_file | sqlite | rocksdb
storage.projection_backend = default | sqlite | rocksdb
```

For v1 auth, the important requirement is:

```text
auth projection backend family == public KV projection backend family
```

The event log can stay on the current borsh file until the storage RFC chooses a
new default.

## Auth Reserved Projection Shape

Auth projection records remain reserved.

Example logical keys:

```text
auth/v1/orgs/<org>/grants/subjects/<subject>/apps/<app>/resources/<resource-id>
auth/v1/orgs/<org>/grants_by_app/apps/<app>/subjects/<subject>/resources/<resource-id>
auth/v1/orgs/<org>/permission_requests/<request-id>
auth/v1/orgs/<org>/audit/by_time/<sequence>
```

These are not public `kv` keys. They are reserved auth projection keys stored in
the shared backend family.

Subject/resource key escaping must be defined before implementation.

## Migration Direction

Storage migration should be phased separately:

```text
Phase S1: auth projection uses same backend family as public KV projection
Phase S2: define logical-store abstraction over the selected backend
Phase S3: add projection rebuild/check commands
Phase S4: move event log default into the shared backend family
Phase S5: add backup/export/restore for log plus projections
```

Auth implementation can begin after S1 is designed. It does not need S4.

## Acceptance Criteria

- Auth correctness works with the current event log.
- Auth reserved projection uses the same physical DB backend family as public KV
  projection.
- Public `ctx.resource.kv` cannot read, scan, or write auth projection records.
- Auth projection can be rebuilt from `auth.*` events.
- Event-log storage unification is tracked as storage work, not as an auth
  correctness blocker.
- A future storage backend switch preserves event ordering and replay identity.

## Open Questions

- Should the selected storage backend be per `TERRANE_HOME` or per capability?
- Should event log and projections live in one DB file or separate files using
  the same backend family?
- What is the first supported shared backend target: SQLite or RocksDB?
- What is the exact logical-store naming scheme?
- How should projection rebuild be exposed in CLI/admin UI?
