# Urbit — the deterministic personal server

The only other platform built on Terrane's core bet: a personal server whose
entire computer is **a deterministic state machine over an event log**.

## Key ideas

- Computer = transactional state machine: an interrupted event never happened;
  the event log is the log of successful transactions. Nock (33-line VM)
  guarantees deterministic, auditable computation; determinism is taken so
  seriously that a jet (optimized native code) diverging from its spec is
  grounds for removal.
- **Gall agents** — userspace apps with persistent state, an event-in /
  effects-out API (ten arms: on-init, on-poke, on-watch, on-peek…). Agents are
  "databases with developer-defined logic / services / state machines".
- **Scries** — read-only requests into a global namespace; can read any
  agent's/vane's state in situ, cannot modify. Remote scries bind immutable
  values to versioned paths with encrypted access control.
- Provenance instead of a formal capability system: every event carries the
  cryptographically-verified source ship; the kernel guarantees eventual
  delivery of pokes/subscriptions (no userspace retry logic).
- Apps distributed as **desks** (agents + front-end published together).

## What it validated for Terrane

- The whole architecture: deterministic replayable core, events as the only
  writes, effects at the edge. Terrane's "one rule" is Urbit's thesis with a
  usable developer surface (JS/Wasm instead of Hoon/Nock).
- Queries/resource reads ≈ scries (pure reads over folded state).
- Desks ≈ [../cap-publish.md](../cap-publish.md) signed bundles.
- Guaranteed-delivery subscriptions between ships ≈ what
  [../cap-sync-v2.md](../cap-sync-v2.md) + [../cap-presence-pubsub.md](../cap-presence-pubsub.md)
  build (with the honest difference that presence is deliberately transient).

## What it exposed

- Nothing unplanned — but it sharpened
  [../cap-history.md](../cap-history.md): Urbit gets time-travel *by
  construction* and surfaces it; Terrane had the same log and no surface.
- Cautionary lesson (not a feature): Hoon/Nock's approachability cost. Terrane
  keeps plain JS + one required verb as the entire app contract.

## Sources

- https://docs.urbit.org/build-on-urbit/userspace
- https://docs.urbit.org/build-on-urbit/app-school/2-agent
- https://docs.urbit.org/build-on-urbit/userspace/remote-scry
- https://docs.urbit.org/urbit-os/kernel/gall/scry
