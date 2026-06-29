# Terrane Auth Design

Terrane's authorization model is **Palantir-style DAC + MAC**. This note defines
the model, scopes the first concrete slice (**user→app confinement**) against the
current engine, and lays out the later axes as a roadmap.

It is the companion to [`cap-design-review.md`](cap-design-review.md): the cap
reshape provides the *seam* (declared surface, a `Subject` on the command path,
`auth` as an ordinary capability read over the bus); this note *populates* it.

Auth's heavy internal surface — identity, sessions, tokens, peer trust, role
bindings — is **deliberately deferred**. Each slice adds only the policy state it
forces. See [Roadmap](#roadmap).

## The model: access = DAC ∧ MAC

Access is a **conjunction**, not a union. To act, you need both:

```text
allowed(subject, action, resource) =
      DAC(subject, resource, verb)        // someone granted you this, at their discretion
   ∧  MAC(subject ⊇ markings(resource))   // you clear every label on it — non-negotiable
```

- **DAC (Discretionary Access Control)** — owner-discretionary ACLs and roles.
  "I shared my list with you as editor." The owner decides; grants are events.
- **MAC (Mandatory Access Control)** — system-enforced **markings**
  (classifications). Even the owner cannot share a `private`-marked resource with
  a subject lacking `private`. MAC is a **ceiling DAC operates under**: it can
  only subtract, never add.
- **Palantir signature feature** — markings **propagate along lineage**. Derive
  data from a `confidential` source and the result is `confidential` too; you
  cannot launder a classification by transforming the data.

**Terrane's structural advantage.** Marking propagation normally sinks MAC
systems because it needs data lineage that almost nobody has. Terrane records
every fact as an event with full lineage — so propagation is just a fold rule
reacting to lineage. This is also what gives the proposed `EventEnvelope.cause`
/`actor` fields a real consumer: **marking propagation follows the cause-lineage.**

## Principals and resources

- **Subjects:** users; their **homes/replicas** (the engine already mints a
  stable `replica.peer`); and — crucially — **apps acting on a user's behalf**.
  An app is a second-level principal: it acts under the *intersection* of the
  user's authority and the app's granted capabilities.
- **Resources:** apps (catalog entries); per-app data (`kv`, `crdt`, `net`
  caches, `model` turns); and the **capabilities/effects** themselves (`net`,
  `model` — "may this app reach the network at all").

## Two MAC axes plus DAC

| Layer | What it controls | Status |
| --- | --- | --- |
| **MAC axis 1 — capability confinement** | which `ctx.resource.*` an app may touch; own-data-only; no effects from JS | **exists today**, enforced structurally |
| **MAC axis 2 — data markings + lineage propagation** | classification labels on apps/data, flowing along the event log | future |
| **DAC — sharing & roles** | owner-discretionary grants; `viewer`/`editor`/`owner` per app; checked at dispatch and at sync admission | future (user→user) |

**The gate** computes `DAC ∧ MAC` on the command/sync path, as pure reads over
folded policy state — **never in `fold`, never replayed.**

---

# v1: user→app confinement

The first slice. It protects the user's apps from each other and from
**harness-generated code** the user does not fully trust.

## What already exists (MAC, structural, app-can't-escape)

The confinement ceiling is enforced *by construction*, not by checks an app could
slip past:

- **Capability allowlist** — the host installs only the namespaces the bundle
  declared; undeclared caps are *absent* from `ctx.resource`, so the app can't
  name them (`rust/crates/terrane-core/src/cap/host.rs:283-298`). `todo`'s
  manifest is literally `"resources": ["kv"]`.
- **Data isolation** — every backend write is force-scoped to the running app:
  `args.insert(0, self.app.clone())` (`cap/host.rs:197`). An app *cannot* target
  another app's data — structurally impossible, not checked.
- **No effects from JS** — backends can't trigger `net`/`model` effects
  (`cap/host.rs:213`); `eval`/`Function` are nulled, with memory/stack/wall-clock
  limits (`cap/host.rs:351`).
- **Default-deny** — absent `resources` → none, least privilege
  (`cap/host.rs:486`).

That is textbook MAC: a mandatory, non-discretionary ceiling the app runs
underneath and cannot widen.

## The one hole: request *is* grant

Today `manifest.resources` is **both** the app's request *and* its grant. The app
declares `["kv","net"]` and the host hands it exactly that — **the user is never
asked.** For hand-written apps that is fine. For **harness-generated apps** it is
the whole risk: generated code declares its own capabilities and self-approves.
There is no DAC.

## The DAC + MAC split for user→app

| | What it controls | Who decides | Grantable away? |
| --- | --- | --- | --- |
| **MAC (the ceiling)** | own-data-only, undeclared-absent, no-effects-from-JS, sandbox limits | the system, fixed | **No** — not even by the user |
| **DAC (the dial)** | *which* requested caps this app actually gets (`kv`, `crdt`; later `net`/`model`) | the **user**, per app | Yes, and revocable |

The mental model is the **mobile-app permission prompt**: the app *requests*
capabilities, the user *grants* a subset, the system enforces the intersection,
and the app can never exceed it. Terrane's enforcement is already built — auth
only inserts the user's consent between request and grant.

## The gate (a one-line insertion)

Effective surface becomes the **intersection**, not the manifest:

```text
install ns  iff  ns ∈ manifest.resources(app)   // MAC: app declared it  (already enforced)
              ∧  ns ∈ granted(user, app)          // DAC: user consented   ← the new line
```

It slots in exactly where the host already filters namespaces
(`cap/host.rs:287-297`): today it installs `manifest.resources`; auth makes it
install `manifest.resources ∩ granted`. Ungranted *or* undeclared → absent, the
same mechanism. The gate runs at **install time, per run** — never in `fold`,
never on replay.

## Lifecycle — all events, off the replay path

```text
app install         → app declares wants (manifest.resources)
first run / prompt   → user grants a subset      → auth.granted   (event, folded)
run                  → host installs declared ∩ granted, app runs confined
user revokes         → auth.revoked              (event, folded)  → next run narrower
```

- **Grants are events** → folded into `auth` state → replayable, and *what an app
  was ever allowed to do is reconstructable from the log* — the audit story falls
  out for free.
- **The check is not replayed.** Option-A replay re-folds the run's recorded
  `kv.*` events (`cap/host.rs:8`); JS never re-runs, so the confinement decision
  is not even on the replay path. `replay_matches` (`lib.rs:475`) stays intact.
- **Policy hook:** harness-generated apps start with **zero grants** — every
  capability requires explicit user consent. That is the confinement payoff for
  the thing that motivated it.

## How v1 rides the reshape

User→app confinement is nearly a **standalone slice** on top of the reshape:

1. **`auth` capability** — holds `granted(user, app) → {namespaces}`, folded from
   `auth.granted` / `auth.revoked`. An ordinary cap; the reshape's bus is how the
   host reads it.
2. **`Subject` into `host.run`** — the gate needs "which user." This is the
   *only* place `Subject` is load-bearing for v1, and `host.run` is a single path
   (`lib.rs:400`) — far smaller than threading `Subject` through all ~100 dispatch
   sites (that is user→user's bill, later).
3. **Intersect at install** — the one-line change above.

## Deferred from v1

- markings + lineage propagation (MAC axis 2);
- making `net` / `model` grantable to apps (and per-domain scoping of `net`);
- the whole user→user DAC (roles, sharing, sync admission);
- any identity / session / token infrastructure beyond a single local owner
  `Subject`.

---

# The auth contract (what cap must expose)

This is the seam the reshape must provide. Writing it down is the acceptance test
for the reshape: if these hooks can't be expressed cleanly, the reshape is wrong
*on paper*, before any cap is touched.

1. **A declared surface.** The capability manifest declares each command with a
   **DAC action class** (`read` / `write` / `admin`). That is *all DAC needs* — a
   command's class versus the subject's role on the target app. (MAC needs
   nothing from the manifest; it rides resource markings + subject clearances +
   lineage.)
2. **A `Subject` reaching the gate.** It enters at the host boundary
   (`host.run` for v1; `dispatch` / the public `InvokeRequest` contract for
   user→user) and rides the command context into the gate.
3. **`auth` as an ordinary capability.** Policy state (grants, later marking
   grants) folded from events, read by the gate over the bus — no field coupling.
4. **The gate is `DAC ∧ MAC`,** evaluated on the command/run/sync path only.

**Worked slice (v1):** the install-time intersection above — `Subject → host.run
→ gate reads granted(user,app) via the bus → install manifest.resources ∩
granted`. **Worked slice (user→user, later):** gate `app.remove` —
`Subject → dispatch → ctx → gate reads the command's `admin` class from the
manifest and the subject's role from auth → allow/deny → normal decide`.

# Determinism invariants (auth-specific)

1. **The gate runs on the command/run/sync path, never in `fold`.** A permission
   check in `fold` would break replay-identity. This is the load-bearing rule.
2. **Grants and revocations are events**, folded into `auth` state like any other
   — replayable, auditable.
3. **The authorization *check* is never replayed.** Replay applies recorded facts
   directly; it does not re-authorize old commands.
4. **Markings (axis 2) propagate via `fold` along the cause-lineage** — a
   deterministic, replayable derivation. The marking *check*, like all gating,
   stays at the gate.

# The honest limit of MAC in a local-first system

Classic MAC assumes a reference monitor mediates every access. In a distributed
CRDT world there is no global monitor:

- The gate is **sound inside a home you control** and at **sync admission**
  (refuse to send/accept marked data to/from an uncleared peer).
- But once a malicious replica holds the bytes, nothing forces it to honor
  markings. **MAC here is a policy boundary — it stops accidents and
  honest-but-curious peers; it is not cryptographic confinement against a hostile
  home.**
- The upgrade, if ever needed, is **per-marking envelope encryption**: encrypt
  marked event payloads so only holders of the marking key can read them.
  Feasible precisely because data is event-structured — but a large step, and not
  first.

# Roadmap

1. **v1 — user→app confinement.** Consent intersection at install. `auth` cap +
   `Subject` into `host.run`. (This note.)
2. **v2 — user→user DAC.** Roles (`viewer`/`editor`/`owner`), sharing, sync
   admission. Forces `Subject` through `dispatch` + the `InvokeRequest` contract,
   and the manifest's DAC action classes.
3. **v3 — MAC axis 2: markings + propagation.** Classification labels flowing
   along the event lineage. Forces the `EventEnvelope` lineage fields and a
   `declassify` (audited, owner-only) escape valve for label creep.
4. **v4 (if ever) — per-marking encryption.** Cryptographic MAC across hostile
   homes.

Each step adds only the policy state and plumbing it forces. Nothing earlier
pre-builds a later step's surface.

# Non-negotiable invariants

1. Access is `DAC ∧ MAC` — both required, MAC is the ceiling DAC cannot exceed.
2. MAC is non-discretionary: no grant, not even the owner's, widens it.
3. The gate is on the command/run/sync path; authorization never runs in `fold`
   and is never replayed.
4. Grants, revocations, and markings are events — folded, replayable, auditable.
5. Markings propagate along lineage; declassification is an explicit, audited,
   owner-only event.
6. Local-first MAC is a policy boundary at the gate and at sync admission, not
   cryptographic confinement — until per-marking encryption is added.
