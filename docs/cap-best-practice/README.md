# Building a capability

How to add a capability crate under `rust/crates/terrane-cap-<name>/` — the
contract, the golden path, and the lessons already paid for in review. For
workspace-wide rules `CLAUDE.md` is authoritative; this folder expands the
capability-specific parts. Architecture context: the repo
[README](../../README.md) and [ARCHITECTURE.md](../../ARCHITECTURE.md).

Everything a capability does happens inside the one shape:

```
Request ──▶ decide ──▶ [Event] ──▶ commit (log) ──▶ broadcast fold ──▶ State
                                        │                               │
                                        └── replay must reproduce ──────┘
```

## TL;DR checklist

1. [ ] **Design** — confirm it's a capability, pick a shape (pure / effectful /
       runtime / event-only / resource-only / projection), name the namespace,
       commands, and event kinds → [01-design.md](01-design.md)
2. [ ] **Know the contract** — decide/fold split, broadcast fold, replay
       identity, typed errors, registry validation → [02-contract.md](02-contract.md)
3. [ ] **Create the crate** — copy `terrane-cap-net`, rename; wire the four
       touch points (workspace members, core dep, `State` field + both
       `get`/`get_mut` arms, `default_registry()`) → [03-skeleton-and-wiring.md](03-skeleton-and-wiring.md)
4. [ ] **Interact without coupling** — bus queries, event subscriptions
       (`app.removed` cleanup!), reserved-KV projections → [04-cross-capability.md](04-cross-capability.md)
5. [ ] **Effects/runtimes/queues if non-pure** — `Effect` variant + `EdgeRunner`
       arm, runtime host writes, or an explicit async request queue; cap-owned
       event constructors; run-once-record-result → [05-effects-and-runtimes.md](05-effects-and-runtimes.md)
6. [ ] **Permissions** — resource methods + `namespace_v1` grant spec; classify
       new commands in `public_authz.rs`; audit for bypass side-channels → [06-permissions-and-policy.md](06-permissions-and-policy.md)
7. [ ] **Tests, four layers** — `src/tests.rs`, `tests/capability.rs`, engine
       tests with `replay_matches()`, binary e2e (`#[ignore]` if effectful) → [07-testing.md](07-testing.md)
8. [ ] **Public surface** — decide CLI/MCP/HTTP exposure, verify contract export,
       smoke capability discovery, keep old logs replayable → [08-public-surface-and-release.md](08-public-surface-and-release.md)
9. [ ] **Docs + done** — `doc()`/`describe()`, regenerate `APP_API.md`, sweep
       the MCP docs, run the full gate, commit small and green → [09-docs-and-done.md](09-docs-and-done.md)

## Case studies

Worked end-to-end applications of the steps above — read the matching numbered
file first, then the case study.

| Doc | Builds |
|---|---|
| [10-case-study-hybrid-search.md](10-case-study-hybrid-search.md) | A `search` capability — BM25 + dense-vector hybrid search (RRF) as a rebuildable KV projection: which library, where embeddings come from, and why the index is not replay-critical state |

## Reference crates

Read code before writing code — each of these is the canonical example of one
shape:

| Crate | Read it for |
|---|---|
| `terrane-cap-net` | The minimal complete capability; the effect pattern; cap-owned event constructors |
| `terrane-cap-kv` | The mature module split; resources + grants; storage backends; `app.removed` cascade |
| `terrane-cap-replica` | Idempotent mint-once effect, guarded on both decide and fold sides |
| `terrane-cap-auth` | Many commands/events; reserved-KV projection alongside a state slice; trust gating |
| `terrane-cap-relational-db` | Pure projection over KV — no own state, no own events, empty fold |
| `terrane-cap-builder` | Event-only (no commands) |
| `terrane-cap-build` | Resource-only (no commands, no events, no state) |
| `terrane-cap-js-runtime` | The runtime shape (`run_runtime`) |

## The three invariants (never break these)

1. **Replay identity** — replaying the log reproduces identical `State`; every
   engine test asserts `core.replay_matches()`.
2. **Broadcast fold, no coupling** — capabilities react to each other's events,
   never call each other; unknown event kinds fall through to `Ok(())`.
3. **Effects at the edge** — anything non-deterministic runs once outside the
   core and enters the log as a recorded event.
