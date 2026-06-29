# Auth Implementation Plan

This turns [`auth-design.md`](auth-design.md) and its review
([`auth-design-review.md`](auth-design-review.md)) into an implementation
sequence, sharpened by the decisions reached in design discussion.

It implements **only v1: user→app capability confinement** — closing the
"request is grant" hole so app code (especially harness-generated code) cannot
self-approve its own resource surface. The full `DAC ∧ MAC` model stays roadmap;
see [`auth-design.md`](auth-design.md).

## Scope

- **In:** an `auth` capability that records resource grants per app; a single
  gate that installs `manifest.resources ∩ granted` for each app run; default-
  deny for generated apps; preview parity; tests proving replay-identity.
- **Out (deferred, by decision):** any user-`Subject` plumbing through
  `dispatch`, actor-on-grant, `EventEnvelope`/lineage, markings (MAC axis 2),
  user→user DAC, denied-attempt audit, per-marking encryption, and the
  effects-from-backend mechanism that `net`/`model` grants would need (see
  [Realizing the teeth](#realizing-the-teeth)).

## Settled decisions (these supersede the earlier docs)

1. **Order:** the cap reshape ships first (per
   [`cap-implement-plan.md`](cap-implement-plan.md), Phases 1–4); auth is the
   slice that follows. Auth builds on the manifest + query bus + host runtime
   service rather than a typed-state shortcut.
2. **Grant key is `(app, resource_namespace)`.** In user→app, the *confined
   principal is the app itself*. There is **no user-`Subject` in v1** — `host.run`
   already has the app id, and top-level grant commands are owner-authorized by
   default (single user). This supersedes both `auth-design.md`'s "Subject into
   host.run" and the review's `Subject::LocalOwner`/`App`. The whole `Subject`
   abstraction belongs to user→user (v2).
3. **One gate chokepoint:** the host runtime service's resource-surface
   construction (today `cap/host.rs:287-298`). The effective surface is
   recomputed **every run**, not cached at install — so revocation narrows the
   next run.
4. **Never gate in `decide`.** `decide` is shared between app runs (`RunAccum`)
   and top-level CLI (`terrane kv set`). A grant check in `decide` would wrongly
   gate the owner's own CLI. The check lives only at surface construction; an
   ungranted namespace is simply *absent* from `ctx.resource`, exactly like an
   undeclared one today.
5. **`auth` reacts to `app.removed`** and drops that app's grants — otherwise an
   `app remove` + `app add` of the same id silently inherits old grants (a
   confinement bypass, since app ids are caller-supplied and stable).
6. **Default-deny**, with a dev/test escape hatch so checked-in examples don't
   break.
7. **Preview uses the same rule** — App Builder runs *generated* code, so it is
   the primary threat surface, not an afterthought.

## Prerequisites (from the cap reshape)

- **Manifest (Phase 1)** — an app's *requested* resources are the bundle's
  `manifest.resources`; the gate intersects against grants. Capability resource
  specs name the gatable namespaces.
- **Context + query bus (Phase 2)** — the gate reads folded `auth` grants over
  the bus, with typed `State` kept. (`auth.decide` also reads `app.exists` over
  the bus, like `kv` does.)
- **Host as engine service (Phase 4)** — the gate lives in the host runtime
  service; that service owns surface construction. Logically the intersection is
  the same one line whether or not Phase 4 has landed, but under the settled
  order it lands in the service.

Auth v1 enforcement does **not** touch `dispatch`/`decide`.

## v1 data model

A new `auth` capability, registered in `default_registry` (`lib.rs:180-193`). Its
state mirrors `KvState`'s shape (`cap/kv.rs:13-16`):

```rust
// granted resource namespaces per app
pub struct AuthState {
    pub grants: BTreeMap<AppId, BTreeSet<String>>,
}
```

```text
commands   auth.grant  <app> <resource_namespace>
           auth.revoke <app> <resource_namespace>
events     auth.granted { app, resource }
           auth.revoked { app, resource }
query      auth.grantsForApp(app)   -> StringList   // for the gate
           auth.requestedForApp(app) -> StringList  // manifest.resources, for prompts
subscribes app.removed -> drop grants for that app
```

Semantics:

- `auth.grant` requires the app to exist (`ctx.bus.query("app","exists")`).
  Granting an already-granted namespace is **idempotent**. `auth.revoke` is
  **idempotent** too — the post-state is "not granted" regardless of prior state.
- No `resource_api`: `auth` is **not** app-facing. An app cannot read or change
  its own grants. Grants are an owner/host concern.
- `auth.grant`/`auth.revoke` are owner-authorized; in single-user v1 that is
  unconditional. user→user will gate them.

## The gate

```text
effective(app) = manifest.resources(app) ∩ granted(app)     // recomputed per run
```

- **Location:** host runtime service surface construction (`cap/host.rs:287-298`).
  Today it installs `manifest.resources`; the change is to intersect with
  `granted(app)` read from folded `auth` state (via the bus).
- **Denial UX:** ungranted → namespace absent from `ctx.resource`. App JS calling
  an absent `ctx.resource.<ns>` throws inside JS and is caught as a runtime error
  — identical to undeclared-resource behavior today. No new failure path.
- **`decide` stays grant-unaware** (decision #4).
- **Manifest growth is safe by construction:** because the surface is recomputed
  from the *current* manifest each run, an app that updates `["kv"]` → `["kv",
  "crdt"]` does **not** auto-acquire `crdt` — the new namespace is ungranted until
  separately approved. Hosts can surface `requested − granted` as "newly
  requested." No grant migration needed.

## Default policy and dev ergonomics

```text
harness-generated app      → zero grants until the user approves
installed third-party app  → zero grants until the user approves
checked-in / dev app       → TERRANE_DEV_ALLOW_REQUESTED_RESOURCES=1 grants
                              requested resources at run time (dev/test only)
```

- Production default is **deny-until-granted**.
- The dev hatch keeps the repo's example apps and unrelated host tests runnable
  without seeding grants.
- **Auth tests seed explicit grants** (via `auth.grant`) so they exercise the
  real gate, rather than relying on the hatch.

## Phases

### Phase A — the `auth` capability (state only, no enforcement)

Grants are recorded and folded; nothing is gated yet.

1. Add `cap/auth.rs`: `AuthState`, `auth.granted`/`auth.revoked` events,
   `auth.grant`/`auth.revoke` commands, fold, `app.removed` subscription, and the
   `grantsForApp`/`requestedForApp` queries. Register it.
2. Add the `auth` slice to `State` and a manifest (Phase-1 style).
3. `auth.grant` validates `app.exists` over the bus; idempotent grant/revoke.

Validation: cap-level tests for grant, revoke, idempotency, and **app-removed-
drops-grants**; `cargo clippy --all-targets -- -D warnings`.

### Phase B — turn on the gate

Confinement goes live.

1. In the host runtime service, intersect `manifest.resources` with
   `grantsForApp(app)` at surface construction.
2. Recompute per run (no install-time caching).
3. Leave `decide`, write app-scoping (`cap/host.rs:197`), and the no-effects rule
   (`cap/host.rs:213`) untouched — structural MAC stays as-is.

Validation: an ungranted requested resource is absent from `ctx.resource`; a
revoke narrows the next run; top-level `terrane kv set` is unaffected by app
grants.

### Phase C — default policy + migrate existing apps/tests

1. Implement default-deny and the `TERRANE_DEV_ALLOW_REQUESTED_RESOURCES` hatch.
2. Decide each checked-in app's path: dev hatch in tests, or an explicit
   first-run grant.
3. Update existing `host.run` tests that assume auto-granted resources.

Validation: full workspace tests green with the policy on; generated-app default
is zero grants.

### Phase D — preview parity

1. App Builder preview computes the same `requested ∩ granted` against draft /
   temporary preview grants.
2. Core/host exposes `requested − granted` (pending) so a host can prompt; the
   **prompt itself lives in hosts** (CLI / macOS / web), never in core.

Validation: preview of generated code installs only granted namespaces; pending
requested resources are reportable.

### Phase E — replay & confinement test suite

Prove the invariants:

1. Grant → run → revoke, then replay reproduces state (data written, grant
   absent).
2. `host.run` replay reruns no JS and re-checks no auth.
3. App removed then re-added inherits **no** grants.
4. Ungranted requested resource absent from `ctx.resource`.
5. Revocation narrows the next run.

Validation:

```sh
cd rust && cargo test --workspace --locked && cargo clippy --all-targets -- -D warnings
cd ../host/cli && cargo test --locked
# macOS App Builder / preview E2E for Phase D
```

## Acceptance criteria

The v1 slice is done when all hold:

1. `auth` records and folds `(app, resource_namespace)` grants/revocations.
2. App backend execution computes `manifest.resources ∩ granted(app)` **per run**.
3. Ungranted resources are absent from `ctx.resource`, like undeclared ones.
4. Harness-generated apps receive no grants by default.
5. Grants and revocations replay deterministically.
6. `host.run` replay is unchanged: JS is not rerun, auth is not rerun.
7. Structural confinement intact: app id force-scoped on writes; JS cannot
   trigger effects.
8. Test: revocation narrows the next run.
9. Test: grant/run/revoke replay reproduces state.
10. Test: an ungranted requested resource is absent from `ctx.resource`.
11. Test: app removed then re-added inherits no grants.
12. Test: top-level CLI dispatch (`kv set`) is unaffected by app grants (gate is
    not in `decide`).
13. Test: a manifest that grows its requested set does not auto-grant the new
    namespace.
14. Preview applies the same effective-resource rule.

## Realizing the teeth

The gate is **resource-namespace-generic** — any resource a cap exposes is
grantable by the same mechanism with no new code. But its security value depends
on *which* resources apps can reach:

- **Today** only `kv` and `crdt` are app-reachable (both pure-`Commit`, local
  data). So v1 builds and proves the consent machinery on low-stakes resources.
- **A future `rel`/`db`-on-kv** cap is pure-`Commit` too, so it is gated by this
  same auth with **zero** extra work — it drops straight in.
- **`net`/`model`** are the high-stakes capabilities, but they are
  effect-producing and effects are barred from backends (`cap/host.rs:213`).
  Exposing them to apps needs an **effects-from-backend mechanism** — a separate
  feature, not part of this plan. When it lands, those namespaces become grantable
  for free, and that is when the gate truly bites.

State this so v1's value isn't oversold: v1 closes the self-approval hole and
makes every present and future app resource consent-gated; the network/LLM
confinement payoff arrives with effects-from-backend.

## Determinism invariants (auth)

1. The gate runs on the command/run path, **never in `fold`**, and is never
   replayed (preserves `replay_matches`, `lib.rs:475`).
2. Grants and revocations are events — folded, replayable, auditable.
3. Replaying a past run's recorded writes does **not** re-authorize; replay folds
   recorded facts (so grant→run→revoke replays to "data present, grant absent").
4. `app.removed` drops grants in `fold` — deterministic, and the reason id-reuse
   cannot re-grant.
5. Any non-determinism a future resource introduces (e.g. a `rel` autoincrement
   id) is resolved in `decide` and recorded in the event, never produced in
   `fold` — the `crdt` pattern (`cap/crdt.rs:11-13`).

## Deferred until forced

- user-`Subject` threaded through `dispatch` / the `InvokeRequest` contract
  (user→user DAC, v2);
- actor-on-grant and `EventEnvelope { actor, cause, … }` (audit / markings, v3);
- markings + lineage propagation (MAC axis 2, v3);
- denied-attempt audit events;
- per-marking encryption (v4);
- effects-from-backend (prerequisite for `net`/`model` grants having teeth).

## Corrections folded in from the design + review

- **Per-run recompute**, not install-time caching (so revocation works).
- **No user-`Subject` in v1** — the app is the principal; key on `(app,
  resource)`.
- **`app.removed` drops grants** — new, prevents id-reuse re-grant.
- **Gate at the host chokepoint, not `decide`** — keeps top-level CLI ungated.
- **"Full lineage" overclaim corrected:** today the log is `{kind, payload}`
  (`domain.rs:35`) with no actor/cause; it is the right *place* to attach lineage
  later, not a property it has now.
- **Generated-app + dev defaults defined**, so existing examples have a path.
- **Preview elevated** from open question to a v1 requirement.
