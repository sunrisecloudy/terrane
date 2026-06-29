# Review: Terrane Auth Design

A review of [`auth-design.md`](auth-design.md) against the current capability
reshape discussion, [`cap-design-review.md`](cap-design-review.md), and the
current engine paths in `rust/crates/terrane-core/src/{lib.rs,cap/host.rs}`.

## Verdict

The design is directionally strong. The most important rule is correct and
should remain load-bearing:

```text
authorization gates commands/runs/sync,
events are recorded facts,
replay never re-authorizes old facts.
```

The v1 scope is also the right first slice: user-to-app confinement for generated
apps is small, urgent, and already mostly supported by the host runtime. The
current engine already has structural MAC-like confinement for app backends:

- resource namespaces are only installed if listed by the bundle manifest
- backend writes are force-scoped to the running app id
- effects from JS are rejected
- `eval` and `Function` are removed from the QuickJS global scope

The main risk is not determinism. The main risk is **scope creep by terminology**:
the document introduces a full DAC + MAC model, but v1 only needs capability
consent for app code. Keep the big model as roadmap, but implement the v1 gate as
a narrow, concrete slice.

## What Is Right

### Replay placement is correct

The design correctly keeps auth outside replay. Grants and revocations are
events, but the permission check is not replayed. This preserves the current
`replay_matches` invariant, where replay folds log records into fresh state and
compares the result to live state.

### The first slice is real

The "request is grant" hole is the right v1 target. Today app manifests declare
resources, and the runtime installs exactly those declared resources. For
harness-generated apps, that means generated code can effectively self-approve
its own resource surface. User consent is the missing layer.

### The MAC/DAC split is useful

The split is a good product model:

- MAC ceiling: app cannot escape own data, cannot access undeclared resources,
  cannot call effects from JS.
- DAC dial: user grants a subset of the app's requested resources.

That maps well to a mobile-style permission prompt and should be understandable
to users.

### Auth as folded state is right

An `auth` capability that owns grant/revoke events and exposes policy reads over
the capability query bus fits the cap design. The gate should read `auth` state,
not import auth internals or special-case auth storage.

## Corrections Before Implementation

### Clarify "install time, per run"

The design says the gate runs at "install time, per run." That phrase is doing
too much. The implementation should distinguish:

```text
manifest.resources = requested resources, read from the app bundle
auth grants        = user-approved resources, folded from auth events
effective surface  = requested ∩ granted, computed for each run
```

If the surface is only computed at install time, revocation will not narrow a
future run. The design also says revocation narrows the next run, so the actual
rule should be:

```text
compute manifest.resources ∩ granted(user, app) at every host.run / preview run
```

Install can record or inspect the requested surface, but run-time resource
installation should use the current folded grants.

### Define the local owner subject

v1 says `Subject` only needs to reach `host.run`, which is mostly true for app
execution. But grants themselves also need an actor.

The document should define the v1 subject model explicitly:

```text
Subject::LocalOwner
Subject::App { app_id, owner: LocalOwner }
```

That avoids pulling in sessions/users/tokens while still letting grant/revoke
events carry a meaningful actor later.

### Separate app confinement from user commands

The existing confinement applies to app backend code, not to top-level user
commands. A local user running `terrane kv set ...` or `terrane net fetch ...`
is not constrained like an app.

The design should state:

```text
v1 gates app runtime resources, especially host.run and preview execution.
top-level user dispatch remains owner-authorized until user-to-user DAC lands.
```

That keeps the first implementation small and avoids pretending the whole CLI
has a finished auth model.

### Define generated-app defaults

The policy hook says harness-generated apps start with zero grants. Good. It
also needs a rule for existing and hand-written apps.

Recommended v1 defaults:

```text
harness-generated app: zero grants until user approves
existing checked-in/dev app: explicit local-owner trust path, or legacy allow in dev mode
installed third-party app: zero grants until user approves
```

Without this, the first implementation will get stuck deciding whether current
examples should break.

### Avoid committing EventEnvelope early

The MAC lineage story needs `actor` and `cause`, but those are permanent log
format commitments. The auth design correctly places markings in v3; keep
`EventEnvelope` there too.

For v1, grant/revoke events are enough. Do not add actor/cause/cap-version
envelopes until audit, sync admission, or markings force them.

### Be precise about "full lineage"

The design says Terrane records every fact as an event with full lineage. Today
Terrane records events, but not full actor/cause lineage. The stronger statement
will only become true after the EventEnvelope work.

Suggested wording:

```text
Terrane's event log gives us the right place to attach lineage later.
```

## V1 Acceptance Criteria

The first auth slice should be considered done when these are true:

1. `auth` capability records and folds grants/revocations for `(subject, app,
   resource_namespace)`.
2. App backend execution computes `manifest.resources ∩ granted(subject, app)`.
3. Ungranted resources are absent from `ctx.resource`, exactly like undeclared
   resources are absent today.
4. Harness-generated apps receive no grants by default.
5. Grants and revocations replay deterministically.
6. `host.run` replay remains unchanged: JS is not rerun, auth is not rerun.
7. Existing confinement remains structural: app id is still force-scoped on
   writes, and JS still cannot trigger effects.
8. There is a test showing revocation narrows the next run.
9. There is a test showing replay after grant/run/revoke reproduces state.
10. There is a test showing an ungranted requested resource is absent from
    `ctx.resource`.

## Suggested Implementation Sequence

1. Add the cap manifest seam from `cap-design-review.md`: command/resource/query
   declarations with a DAC action class slot.
2. Add the query bus seam needed for `auth` reads, while keeping typed `State`.
3. Add `auth` state and events:
   - `auth.granted`
   - `auth.revoked`
4. Add the minimal v1 `Subject` only on the app runtime path.
5. Gate host resource installation with requested/resources intersection.
6. Add generated-app default-deny behavior.
7. Add tests for grant, revoke, absent resources, and replay.
8. Only later thread `Subject` through general `dispatch` for user-to-user DAC.

This keeps auth aligned with the cap reshape without paying the full user/user
or MAC-lineage cost early.

## Open Questions

### Where does the permission prompt live?

The design says first run prompts the user, but it does not specify whether the
prompt belongs to macOS, web, CLI, or a shared host API. The core should expose
the pending requested resources, but the prompt itself should live in hosts.

### What is the dev-mode policy?

The repo has checked-in example apps and local development workflows. Decide
whether dev apps are auto-granted by local-owner trust, prompted once, or run
with a `TERRANE_DEV_ALLOW_REQUESTED_RESOURCES`-style escape hatch.

### Are grants app-versioned?

If an app updates its manifest from `["kv"]` to `["kv", "crdt"]`, the existing
`kv` grant should not silently approve `crdt`. Namespace-level grants handle
this if the effective surface is recomputed from current manifest resources, but
the UX should make newly requested resources visible.

### Do denied attempts become audit events?

Not needed for v1 determinism. If added, denied-attempt records should be audit
events that do not affect app state, and they should be designed carefully to
avoid leaking sensitive prompt/data content.

### How does preview auth differ from installed app auth?

App Builder preview runs generated code before install. Preview should probably
use the same effective resource rule, but against draft/granted preview state or
an explicit temporary grant.

## What To Carry Forward

Keep these from `auth-design.md` unchanged:

- access is `DAC ∧ MAC`
- the gate never runs in `fold`
- auth checks are never replayed
- grants and revocations are events
- user-to-app confinement is v1
- markings and lineage propagation are future work
- local-first MAC is a policy boundary, not cryptographic confinement

The design is strongest when it stays concrete: first make generated apps unable
to self-approve capabilities, then grow the broader DAC/MAC model only when the
next slice forces it.
