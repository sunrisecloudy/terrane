# 05 — Effects and runtimes

The core is deterministic; the outside world is not. Both non-pure shapes
resolve the tension the same way: **the unpredictable thing runs once, at the
edge, and only its recorded result enters the log.** Replay folds events — it
never re-fetches, re-prompts, or re-executes JS.

## Effects

Lifecycle (`net.fetch` end to end):

1. `decide` validates purely and returns
   `Decision::Effect(Effect::HttpGet { app, url })` — no I/O yet.
2. The engine hands the effect to its `EffectRunner`
   (`rust/crates/terrane-core/src/lib.rs`). The real runner is the host's
   `EdgeRunner` (`rust/crates/terrane-host/src/edge.rs`); a core opened without
   one (`NoEffects`) errors on any effect, which keeps engine tests honest.
3. The runner performs the I/O **once** and returns `EventRecord`s.
4. The records are committed and broadcast-folded like any other events.

Rules:

- **The capability owns the event shape even though the runner builds it.**
  Export a constructor and make the runner call it — `net::fetched_event(app,
  url, status, body)` — so the `kind` string and payload struct never leak.
- `Effect` is a closed enum in `rust/crates/terrane-cap-interface/src/abi.rs`.
  A new effect means a new variant there plus a runner arm in `EdgeRunner` —
  that host arm is your fifth wiring point.
- Record the *result*, not the request: fold must be able to rebuild state from
  the event alone.
- Idempotent-by-design where re-running must be safe: `replica.init` returns
  `Decision::Commit(vec![])` when an identity already exists, and its fold
  guards "first identity wins" so even a duplicated event can't re-mint. Use
  the same two-sided guard for any mint-once effect.
- Effectful e2e tests hit the real network/CLIs, so they are `#[ignore]`d with
  a reason ([07-testing.md](07-testing.md)).

## Runtimes

A runtime capability executes app backends (QuickJS, WASM). Lifecycle:

1. `decide` on `js-runtime.run` validates and returns
   `Decision::Runtime(RuntimeRequest { app, input })`.
2. The engine calls your `run_runtime(ctx, request)` with a `RuntimeCtx`
   carrying the bundle source and a `RuntimeHostHandle`
   (`rust/crates/terrane-cap-interface/src/runtime.rs`).
3. Guest code calls `ctx.resource.<ns>.<method>(…)`. Reads go to the owning
   capability's `read_resource`. Writes are routed to the owning capability's
   `decide` — **only `Decision::Commit` is legal inside a runtime**; effects
   and nested runtimes are refused.
4. Writes apply to a working copy immediately (later reads in the same run see
   them), are collected, coalesced (`kv` keeps only the final `set` per key),
   and committed as ordinary events after the run.

This is Option A replay: the log holds only `kv.*`-style events, so replay
rebuilds state without ever re-running JS. A new runtime kind (new language,
new sandbox) is a new capability implementing `run_runtime`; it typically has
no state, no events, and an empty fold — `terrane-cap-js-runtime` is ~80 lines
of `lib.rs`.

## Choosing between them

| Question | Effect | Runtime |
|---|---|---|
| Who is unpredictable? | The world (network, agent, entropy) | The app's own code |
| What gets recorded? | The result, as *your* event kind | The guest's writes, as the *owning caps'* event kinds |
| Where does the impl live? | Host `EdgeRunner` arm | Your `run_runtime` |

Next: [06-permissions-and-policy.md](06-permissions-and-policy.md) — who may call all of this.
