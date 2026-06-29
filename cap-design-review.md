# Review: Terrane Capability Design

A review of [`cap-design.md`](cap-design.md) against the current engine
(`rust/crates/terrane-core/src/cap/{mod,kv,app,host}.rs`, `domain.rs`, `lib.rs`).

Auth is **committed and imminent** (user→app first). Its model and v1 scope live
in [`auth-design.md`](auth-design.md); this review keeps only auth's *seam* with
the cap interface and sequences the reshape so auth slots in cleanly.

## Verdict

The *direction* is right and consistent with where the code already leans, but
the design bundles ~4 separate-risk moves into one note, which fights the
project's "smallest thing that is genuinely the system" rule. Roughly 70% is the
right destination. The problem is the **sequencing**: the migration plan
front-loads the two riskiest, least-forced pieces (opaque `CapStore` at step 2,
auth's full surface woven through the interface) and buries the two safest,
highest-value ones (manifest generalization + query bus).

This review keeps what's right, rejects one piece as written, splits auth into
seam-vs-feature, and proposes a re-ordered, auth-anchored sequence.

## What the exploration found (and how it reshaped the plan)

Two findings from reading the engine sharpened everything below:

- **The cross-cap read surface is two reads, total.** Only `app.exists` (via
  `app.apps.contains_key`, used by `kv`, `crdt`, `harness`, `model`, `net`) and
  `replica.peer` (used by `crdt` only) ever reach across slices. Everything else
  is own-slice. So the query bus abstracts *two* reads — a half-day, not a
  project.
- **The blast radius is at `dispatch`, not `decide`.** `decide` is called in two
  production places: `Core::dispatch` and `RunAccum::write` inside the QuickJS
  run path. The ~100 `dispatch(Request)` / `req(...)` sites never touch `decide`
  directly — they go through the `Core::dispatch` boundary. So a
  `decide`-signature change is still cheap (trait + 10 impls + 2 call sites);
  a `dispatch`-signature change (what a `Subject` needs) is the expensive one.

A third finding shaped auth's scope, not the reshape: **user→app confinement is
~80% built** — the per-app capability allowlist, own-data scoping, and
no-effects-from-JS are already enforced structurally in `cap/host.rs`. See
[`auth-design.md`](auth-design.md).

## What's genuinely right

- **Naming the CQRS phases is honest, not new surface.** Today's
  `decide`→`fold` already *is* command→apply. Calling it that costs nothing and
  clarifies intent. The doc correctly resists collapsing this into a single
  generic `handle()`.
- **Cross-cap reads through a query bus is the real prize.** Today `kv.decide`
  reaches directly into another cap's slice — `state.app.apps.contains_key(&app)`
  (`cap/kv.rs:68`). *That* coupling is what blocks crate-per-cap.
  `ctx.query("app","exists",…)` is the right cut — and it is only two reads wide.
- **Rejecting `CoreState { caps: BTreeMap<CapId, CapState> }`** is correct — that
  just relocates the god-object instead of removing it.
- **Name-tagged `EffectRequest`** matches a TODO already in the code:
  `lib.rs:77-79` literally says "if they proliferate, make them name-tagged like
  events with a runner registry." Aligned.
- **`host` → engine service.** Already half-true: `dispatch` special-cases
  `host.run` because it needs `&mut self` (`lib.rs:400`). Making that explicit
  instead of a fake `Capability` is more honest.
- **Subtle and good:** the command context only hands out a read-only
  `CapBus::query`. A cap therefore *cannot* trigger I/O or sibling commands during
  decide. That preserves the "decide is pure" invariant by construction — worth
  calling out as a designed-in feature, not an accident.

## The load-bearing objection: keep typed `State`, don't adopt opaque `CapStore`

This is the one piece to reject as written.

Today the core correctness check is one line — `fresh == self.state`
(`lib.rs:475`). The entire replay-identity contract rides on typed `PartialEq`
over a typed `State`:

```rust
pub struct State { pub app: AppState, pub kv: KvState, pub crdt: CrdtState, … }
```

A `CapStore { get/put -> Vec<u8> }` throws that away:

- You lose typed structural equality for the replay check — or you reimplement it
  over byte blobs, and byte-equality of independently-serialized state is a
  *stronger, more fragile* claim than structural equality.
- Every hot read in `decide` becomes serialize/deserialize instead of a field
  read.
- The `crdt` slice holds Loro documents containing `f64`; it is `PartialEq`-only
  *on purpose* (`lib.rs:50-53`). It does not fit a `Vec<u8>` blob store cleanly.

The decoupling the doc wants does **not** require `CapStore`. Get it from the
**query bus**: a cap keeps its own typed `KvState` and still exposes `kv.all` over
the bus. The store abstraction only becomes necessary for *dynamic/WASM* caps —
which the doc itself defers to step 12. So `CapStore` is solving a step-12 problem
at step 2.

**Recommendation:** keep typed per-cap state; achieve cross-cap decoupling through
the query bus, not by erasing the types.

## Auth: the seam stays here, the feature lives in `auth-design.md`

Auth is happening — but its *internals* (identity, sessions, tokens, peer trust,
role bindings, the policy model) still do not belong in the cap interface. The
split:

- **In the cap reshape (here):** the *seam* — a manifest that can carry a per-
  command **DAC action class**, a `Subject` that can reach a gate, and `auth` as
  an ordinary capability read over the bus. The reshape must *make these
  expressible*; it does not build them.
- **In [`auth-design.md`](auth-design.md):** the model (`DAC ∧ MAC`), the v1
  user→app slice (consent intersection at install), and the later axes.

The original doc's mistake was threading auth's full surface (a `permission` on
every spec, a core enforcement service, an identity-laden `auth` cap) through the
interface before any gate existed. Keep the seam; let each auth slice add only the
policy state it forces.

### Correction: thread `Subject` *with* auth, not before — the context is the seam

An earlier draft of this review said "do `Subject` now, once." The exploration
shows that is wrong. A `Subject` does nothing until a gate exists, and it must
enter at `dispatch` / the public `InvokeRequest` contract — ~100 sites. Threading
a dead param through all of them now is pure churn with no consumer.

The real "do it once" win is different: **introduce the command/query *context*
now** (it already carries the `CapBus`), so `decide` becomes `decide(ctx, …)`.
That touches only the trait + 10 impls + the one call site. When auth lands,
`Subject` rides *inside* that context — the caps never get re-touched. **The
context is the extension seam; the seam is cheap; `Subject` fills it later.**

Note the asymmetry this exploits: user→app v1 needs `Subject` only at `host.run`
(one path), so it lands without paying the ~100-site `dispatch` bill. That bill
comes only with user→user (v2), when every changed line is meaningfully "add
auth."

## Fine, but "when forced" — not now

| Move | Worth it when | Not yet because |
| --- | --- | --- |
| `Command { name, payload: Value }` vs `args: &[String]` | a non-CLI host needs structured args | MCP/web hosts currently marshal fine; it's a breaking change across every cap + host adapter |
| Name-tagged `EffectRequest` | effects proliferate past the current 5 | the central `Effect` enum is still readable (`lib.rs:80-111`) |
| `EventEnvelope { actor, cause, cap, cap_version }` | sync/audit/versioning has a real consumer | every field is a permanent on-disk log commitment. `cap_version` → versioning; **`cause`/`actor` → MAC marking propagation (axis 2)** — so they land with auth v3, not before |

Each is a correct eventual destination. None should be built preemptively.

## Pull this one *forward*: generalize the manifest

The highest-value, lowest-risk move is buried in the design. `resource_api()` is
already a partial manifest, and it is the single source driving **both** the
runtime bridge and the generated docs (`cap/mod.rs:49-55`) — that "cannot drift"
property is the best pattern in the codebase.

Extending the same declaration to also cover commands / events / queries /
subscriptions lets the registry **validate** a capability's surface, makes
`subscriptions` explicit instead of every `fold` blind-matching every event kind
(today `kv` matches `"app.removed"` by hand — `cap/kv.rs:121`), and gives auth its
**DAC action-class** slot. Pure decoupling, no semantic risk, and it subsumes the
existing `resource_api`.

## Determinism notes (keep these invariants explicit)

The design's non-negotiables match the code and should stay verbatim:

- queries are read-only and **never replayed**;
- effects perform I/O **once** and return events; replay folds the recorded event,
  never re-runs the effect (`lib.rs:25-29`);
- envelope metadata (`actor`, `cause`) must **not** affect `fold`, or
  replay-identity breaks (even when MAC propagation *reads* `cause`, the
  propagation is a fold derivation; the auth *check* stays at the gate).

Two additions worth stating outright:

- the `CapBus` reachable from the command context exposes **only** read-only query
  handlers — never command or effect entry points — so a cap cannot perform I/O
  during decide;
- **the auth gate runs on the command/run/sync path, never in `fold`, and is never
  replayed** (`auth-design.md`, determinism invariants). None of steps 1–3 below
  go near `fold` or the log, so they are replay-safe by construction.

## Recommended sequence (auth-anchored)

1. **Generalize the manifest** (subsumes `resource_api`); registry validates each
   capability's declared surface. Add a per-command **DAC action-class** slot now
   (the seam), even though nothing reads it until auth v2.
2. **Introduce the command/query context + `CapBus`**, and convert the *two*
   cross-cap reads (`app.exists`, `replica.peer`) to `ctx.query(...)` —
   **keeping typed `State`**. This is the `Subject`-injection seam *and* the
   cross-cap decoupling in one cheap change.
3. **Make `host` an explicit engine service** over registry resource specs,
   instead of a special-cased fake `Capability`. (Optional cleanup — fold in only
   if step 2 makes it cheap.)
4. **Then auth, per [`auth-design.md`](auth-design.md):** v1 user→app confinement
   = `auth` cap + `Subject` into `host.run` + the install-time `manifest.resources
   ∩ granted` intersection. Later slices (user→user DAC, markings) follow there.
5. **Leave deferred:** `CapStore`, `Value` commands, name-tagged `EffectRequest`,
   `EventEnvelope` metadata, and dynamic/WASM caps — each until a concrete need
   forces it.

This reorders the original 12-step plan around risk and necessity: two cheap
structural changes first (manifest + context/bus), host-cleanup optional, then
auth as its own staged track — and the one thing the original front-loaded
(`Subject` everywhere) is correctly repositioned as auth's bill, paid one path at
a time.

### Two open judgment calls

- **Query bus now (step 2) vs. at crate-split.** Its full payoff is when caps move
  to separate crates. Within one crate it's indirection for two reads. I'd still
  do it now — it's tiny, it makes the manifest's "queries" surface real, and it is
  the `Subject` seam — but a strict "not yet forced" reading would defer it.
- **Step 3 now vs. with auth.** Pure cleanup, no auth dependency. Fold in only if
  step 2 makes it cheap; otherwise let it ride.

## What carries over unchanged from `cap-design.md`

- The CQRS framing and the explicit command / apply / query / effect phases.
- "Do not centralize state" — core owns log, registry, store, effect runner; each
  cap owns its own schema.
- Cross-cap reads via query API rather than direct field access.
- Caution on dynamic capabilities: built-in Rust crates first; WASM/QuickJS
  app-installed caps only after the built-ins are stable.
- The non-negotiable invariants list (with the `CapBus`-is-read-only and
  gate-never-in-`fold` additions above).
