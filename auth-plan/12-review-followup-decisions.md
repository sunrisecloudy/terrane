# Review Follow-Up Decisions

This document summarizes decisions made after reading
`review-001-auth-plan.md`.

It is a planning artifact only. No code implementation has been started for
these decisions.

## Purpose

Claude's first review accepted the overall auth direction, but identified one
load-bearing gap: the plan did not fully specify how runtime policy is read and
how replay stays deterministic.

This document records the decisions that should be reviewed before coding.

## Locked Decisions

## 1. Runtime Policy Read Path

Decision:

```text
Runtime gate uses a privileged typed authz helper over folded AuthState.
Admin/reporting queries can be built later on top of safe public/internal APIs.
```

The runtime gate should not depend on:

```text
generic QueryValue JSON
direct reserved KV scans
public capability queries
```

Target shape:

```text
RuntimeResourceHost
  has ExecutionPrincipal + app
  calls AuthzService::grants_for_runtime(org, subject, app)
  installs only allowed resource namespaces/selectors
```

Reason:

`QueryValue` cannot currently return grant lists, and even if expanded later,
runtime confinement should use a small typed path that is hard to expose by
accident.

## 2. Auth Event Ownership And Projection

Decision:

```text
Auth owns auth.* events and AuthState.
Auth MUST project to reserved internal storage.
Public ctx.resource.kv never sees auth data.
```

Meaning:

```text
auth.grant command
  -> emits auth.granted
  -> event log stores auth.granted
  -> AuthState folds auth.granted
  -> reserved internal auth projection is updated
```

Do not model auth policy as public KV events:

```text
auth.grant -> kv.set
```

KV and auth may share physical storage infrastructure, but they do not share
semantic ownership.

The storage target for sharing the same physical DB backend family as KV is
tracked separately in `13-storage-target.md`.

## 3. Physical Storage Default

Storage target:

```text
By default, the event log itself should use the same physical DB backend family
as KV and projections.
That default can change later.
```

This is a storage target, not an auth correctness requirement. See
`13-storage-target.md`.

Default storage picture:

```text
same physical backend by default
  event log
    auth.* events
    app.* events
    kv.* events
    ...
  projections
    public app KV
    reserved internal auth records
    auth audit/query indexes
```

The shared physical backend does not collapse capability ownership.

## 4. Deterministic Premium Snapshot Import

Decision:

```text
Premium signature verification and expiry checks happen at import/sync edge
time.
They produce recorded auth facts.
Runtime gate reads folded facts only.
Runtime gate never checks wall-clock or re-verifies signatures.
```

Target flow:

```text
Premium sync/import
  -> verify signature
  -> check valid_from / valid_until
  -> record auth.policy_snapshot.imported
  -> record imported auth.granted / auth.revoked facts

Runtime gate
  -> reads folded AuthState
  -> decides from facts only
```

Reason:

Replay determinism is a hard requirement. Runtime access must not change because
the current wall clock changes during replay or later execution.

## 5. Explicit Execution Principal

Decision:

```text
Runtime requests carry explicit ExecutionPrincipal.
Local v1 default is org:local + user:local-owner + source:local.
```

Suggested shape:

```text
ExecutionPrincipal {
  org: OrgId,
  subject: SubjectId,
  source: PrincipalSource
}

PrincipalSource =
  local
  premium
  agent
  anonymous
```

Examples:

```text
Local user running an app:
  org = local
  subject = user:local-owner
  source = local

My local AI agent running an app:
  org = local
  subject = agent:user:local-owner:<agent-id>
  source = agent

Anonymous preview:
  org = local
  subject = anonymous:user
  source = anonymous

Premium user:
  org = org:<premium-org-id>
  subject = user:<premium-user-id>
  source = premium
```

Reason:

The grant key is `org + subject + app + resource`. The runtime gate cannot be
subject-specific unless subject is carried into runtime dispatch.

## 6. Resource Specs Are Mandatory

Decision:

```text
No capability may expose ctx.resource.<namespace> unless it also exposes a
GrantResourceSpec for that namespace.
No generated app may request a resource namespace that is not backed by a
registered GrantResourceSpec.
```

This must become a code-enforced invariant, not just documentation.

Required interface direction:

```text
Capability
  resource_api() / manifest.resources
  grant_resource_specs()

registry validation:
  if resource_api is non-empty then grant_resource_specs is non-empty
  every exposed resource namespace has at least one GrantResourceSpec

runtime installation:
  if no GrantResourceSpec exists for namespace -> do not install resource

builder/app manifest validation:
  allowed resources derive from registered GrantResourceSpec namespaces
```

Reason:

Resource access without a resource spec creates an unreviewable permission
surface. The admin UI, runtime gate, and permission broker all need the same
spec source.

## 7. Capability-Owned Selector ID And Compatibility

Decision:

```text
Capability defines selector_schema_id, selector_id(selector), validation,
summary, verbs, and compatibility.
Auth stores selector_schema_id + opaque selector_id + full selector JSON.
Unknown future selector schema fails closed.
Known older selector schemas must remain readable or be migrated by recorded
auth facts.
```

Auth owns the grant envelope. The target capability owns selector semantics.

Grant value shape:

```text
namespace
selector_schema_id
selector_id
selector_json
verbs
```

Compatibility rules:

```text
Known current schema -> validate and allow if granted.
Known older schema -> validate with retained old validator, or use recorded
migration facts.
Unknown future schema -> fail closed.
Breaking selector meaning change -> new selector_schema_id.
Additive optional fields -> allowed within same schema only if old readers
safely ignore them.
```

Example:

```text
relational_db owns:
  selector_schema_id = table.v1
  selector_id({ "table": "customers" }) = table.customers
  validate_selector(...)
  selector_summary(...)

auth stores:
  namespace = relational_db
  selector_schema_id = table.v1
  selector_id = table.customers
  selector_json = { "table": "customers" }
```

Auth treats `selector_id` as opaque and does not parse `table.customers`.

## 8. Audit Is Event-Derived

Decision:

```text
Audit is a projection over auth.* events.
Optional reserved internal indexes may exist for query speed.
The event log remains the append-only source of truth.
```

Do not create a separate authoritative KV audit log.

## 9. Manifest Resource Allow-List

Decision:

```text
Generated-app manifest resource validation derives from registered
GrantResourceSpec namespaces.
```

Do not keep a manual allow-list such as:

```text
kv | crdt | document
```

Reason:

Manual allow-lists drift. The code already has signs of drift: planning and MCP
examples mention resources such as `relational_db`, while builder validation
still has a narrower hard-coded list.

## Code-Enforced Invariants To Add Later

These are implementation requirements, not yet implemented:

- A capability with runtime resource methods must declare grant resource specs.
- A capability with grant resource specs must document selector schema,
  selector id behavior, verbs, and compatibility.
- Runtime resource installation must deny namespaces with no grant spec.
- Builder/app manifest validation must use grant spec namespaces.
- Unknown future selector schema must fail closed.
- Old selector schema fixtures must stay valid, or migration facts must exist.
- Runtime gate must be deterministic and use folded AuthState only.
- Public `ctx.resource.kv` must never expose auth records.
- Auth reserved projection must stay hidden from public app KV.

## Items Still Open For Review

These may need another pass before implementation:

- Exact Rust type names for `GrantResourceSpec`, `ExecutionPrincipal`, and
  `AuthzService`.
- Whether `grant_resource_specs()` belongs directly on `Capability` or inside
  `CapManifest`.
- How much selector validation runs in auth commands vs admin UI preflight.
- Exact physical DB abstraction for event log plus projections, tracked in
  `13-storage-target.md`.
- Whether namespace-only v1 uses a built-in `namespace.v1` selector schema for
  every resource capability.
