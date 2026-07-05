# Primitive: actor — provenance on every event

Engine-level change in `terrane-cap-interface` + `terrane-core` (no new cap
crate): every event records **who caused it**. Today `EventRecord` is bare
`{kind, payload}`; the log knows *what* happened but not *whom* to attribute
it to — which makes agent audit ("show me everything this agent did, undo
it") impossible and leaves [cap-history.md](cap-history.md) blind to actors.

## Locked decision (user, 2026-07-05)

**Actor is a required field on the event envelope** —
`EventRecord {kind, payload, actor}` — and **no backward compatibility**:
pre-release, the log format simply changes, with a one-time migration that
stamps all existing events `user:local-owner`. No optional-forever field, no
dual-format code paths.

## Design

- `actor` = the canonical principal string, stamped **by the engine at
  commit** from the request's `ExecutionPrincipal` — capabilities never set
  or see it as an input, so no capability can forge attribution. Forms:
  - `user:<person_id>` (after [primitive-person.md](primitive-person.md);
    `user:local-owner` until then)
  - `agent:<owner>:<id>` (already the agent cap's principal form)
  - `app:<caller-app>` (interop calls, [cap-interop.md](cap-interop.md))
  - `peer:<replica>` (events accepted over sync)
  - `anon` (web-publish visitors), `host` (daemons: scheduler fire,
    automation fire, migrations)
- Org context rides the same stamp: with
  [primitive-org.md](primitive-org.md), the envelope carries
  `{org, subject}` exactly as `ExecutionPrincipal` does — one struct, reused.
- **Replay-identity is unaffected**: actor is data folded like everything
  else; decide/fold stay pure. `describe()` output gains the actor prefix so
  `terrane log` reads as an audit trail for free.

## Migration

1. Bump the log format version (persistence header).
2. `terrane migrate-log`: streams the old log, writes each record with
   `actor: "user:local-owner"`, verifies replay-identity of the folded state
   before/after (must be byte-identical modulo the new field), swaps files
   atomically, keeps `log.bin.pre-actor` until the user deletes it.
3. Hosts refuse to open an old-format log with a typed error naming the
   command. Sync peers must be same-version (pre-release stance).

## What it unlocks (why this goes early)

- [cap-history.md](cap-history.md) gains `filter: actor` and
  `revert --actor agent:…` (undo an agent session) as a natural v2.
- The auth admin console shows *who* requested/consumed every grant.
- Org accountability: member actions in a shared home are attributable.
- Every future cap inherits provenance with zero per-cap work — the reason
  this must land **before** significant data accumulates.

## Implementation plan

1. `EventRecord` struct + borsh envelope change in `terrane-cap-interface`;
   `ExecutionPrincipal → actor string` canonicalization in one place.
2. Engine commit path stamps actor; broadcast-fold reactions attribute to the
   original actor (a reaction is still the actor's consequence).
3. Persistence format version + `terrane migrate-log` (+ replay-identity
   verification step).
4. Sync envelope carries actor verbatim; received events are re-checked:
   a peer may not claim `user:` subjects the local auth state doesn't grant
   it (edge policy, [cap-sync-v2.md](cap-sync-v2.md)).
5. Mechanical test-fixture sweep (every cap test constructs EventRecords).
6. Tests: stamping per principal kind, forge-resistance (capability-supplied
   actor ignored), migration round-trip on a fixture log, describe output.

Gate: `cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals

Cryptographic signing of individual events (person signatures cover
attestations and publishing; per-event sigs are cost without a threat model
yet), actor-based *authorization* (grants do that — actor is attribution,
not permission).

## Decisions to confirm

- **Reactions attribute to the original actor** — recommend yes (causality)
  — alternative: `host` for all broadcast-fold consequences (simpler, loses
  the chain).
- **Keep `log.bin.pre-actor` backup by default** — recommend yes until
  user-deleted — alternative: delete after verified migration.
