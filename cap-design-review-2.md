# Review 2: Capability Implementation Plan

A review of [`cap-implement-plan.md`](cap-implement-plan.md) (Codex's plan) against
the current engine. Companion to [`cap-design-review.md`](cap-design-review.md)
(which reviewed the *design*) and [`auth-design.md`](auth-design.md).

## Verdict

Faithful and well-staged. It keeps typed `State`, scopes the query bus to the two
real cross-cap reads, introduces the command context as the extension seam, holds
`Subject` to `host.run`, and defers the right list. The coupling inventory is
**exactly right** (verified by grep): `app.exists` in `kv`/`crdt`/`net`/`model`/
`harness`, `replica.peer` in `crdt` only.

Green-light Phases 1–2 once the two correctness fixes below (#1, #2) are folded
in. The rest are notes, not blockers.

## Decision (settled): keep Codex's order — reshape first, auth v1 at Phase 5

The open question from review 1 was *clean reshape first* vs *auth v1 first*. The
call is made: **follow Codex's ordering.** Phases 1–4 (manifest → contexts/bus →
subscriptions → host-as-service) land first; **auth v1 (user→app) follows as
Phase 5.**

Consequence — and it's a *good* one: auth v1 then builds on the finished seam (the
manifest's DAC action-class slot, the read-only bus, and the host runtime
service) rather than on a quick typed-state hack. That makes it compose cleanly
with user→user (v2) later. The only watch-item: nothing in Phases 1–4 should bake
in an assumption that blocks the install-time `manifest.resources ∩ granted`
intersection — keep the host's namespace-filter step (`cap/host.rs:287-297`) the
single chokepoint where that intersection will live.

So review 1's finding #3 ("pull auth earlier") is **withdrawn by decision**.

## 1. `decide` has *two* call sites, not one — and the bus must span both

This corrects an error in `cap-design-review.md`, which states `decide` is "called
in exactly one place (`lib.rs:407`)." There are **two** production call sites:

- `Core::dispatch` — `lib.rs:407`
- `RunAccum::write` (inside the QuickJS run path) —

  ```rust
  // cap/host.rs:206-210
  let decision = self.registry
      .get(namespace_of(name)?)?
      .decide(&self.state, name, &args)?;   // ← second site
  ```

Implications for Phase 2:

- The signature change `decide(&State,…) → decide(CommandCtx,…)` touches **both**
  sites.
- At the `RunAccum` site, `&self.state` is the **run's working `State`** (a clone,
  `lib.rs:439`) and the registry is a **fresh per-run `Registry`**
  (`cap/host.rs:110`). So `CapBus` must be constructible over an arbitrary
  `(&Registry, &State)` pair — **not** bound to `Core`. The plan's task 3 permits
  this ("`CapBus` for `Registry` or a small `EngineBus` wrapper"); make it a
  *requirement*: the bus is `(registry, state)`-shaped.
- Concretely, when a backend calls `ctx.resource.kv.set`, `kv.decide` resolves
  `app.exists` — that must work over the *run's* working state, through the bus,
  mid-run.

This is the most likely thing to wedge the refactor mid-flight. It deserves an
explicit sentence in Phase 2.

## 2. Phase 1 "validate duplicate event names" will false-positive on `app.removed`

Four caps **fold** `app.removed` (`kv` at `cap/kv.rs:121`; `crdt`, `net`, `model`
similarly) — the broadcast-subscription pattern. Naive "duplicate event name"
validation flags all of them.

Validation must run on **owned/emitted** events only (one declaring owner per
event kind); *subscriptions* (Phase 3) then reference an already-declared kind.
The plan separates `events` from `subscriptions` structurally, so the fix is just
to state that the Phase 1 check is over owned events, not mentions — otherwise
Phase 1 and Phase 3 contradict each other.

## 3. `Subject` delivery via the API hosts (medium)

Phase 5 scopes `Subject` to `host.run` — fine for the CLI (implicit local owner).
But MCP and web invoke `host.run` through the public `InvokeRequest` contract
(`terrane-api`), not the CLI. v1 needs a defined **default-local-`Subject`** path
there, or those hosts can't run gated apps. Small, but easy to discover late —
state it: "API hosts pass a default local `Subject`."

## 4. Small stuff (note, don't block)

- **Two value enums.** `ReadValue` (backend resource reads, `cap/mod.rs:59`) and
  the new `QueryValue` overlap heavily. Fine to keep separate now (different
  consumers: JS backend vs Rust caps), but flag the eventual convergence so they
  don't drift into a third.
- **`QueryValue` has unused variants.** The two real queries need only `Bool` and
  `U64(Option<u64>)` (`replica.peer` is a Loro `PeerID` = `u64`). `String` /
  `StringMap` / `StringList` are forward-looking — harmless, but a strict "start
  with what's needed" would trim them until a query returns them.
- **Phase 2 edits code Phase 4 relocates.** The `RunAccum` decide call (#1) is
  modified in Phase 2, then moved into the host service in Phase 4. Acceptable
  churn; noted so it isn't a surprise.
- **Trait sketch (lines 56–76)** shows `decide(ctx, …)` under the Phase-1 heading,
  though decide doesn't change until Phase 2. Cosmetic — it's the end-state shape
  — but add "end state; `decide` changes in Phase 2."

## Cross-doc correction

`cap-design-review.md` should have its "decide is called in exactly one place"
line corrected to two call sites (see #1). It's a one-line fix; flagged here so
the two review docs don't disagree.

## Net

Structurally sound and faithful to the design review. Adopt Codex's order
(reshape → auth v1). Fold in #1 and #2 before starting Phase 2; keep #3 in view
for Phase 5; treat #4 as cleanup. With those, Phases 1–2 are ready to implement.
