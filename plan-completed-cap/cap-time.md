# Capability: `time` — replay-safe "now" (DRAFT)

New crate `rust/crates/terrane-cap-time/`, namespace `time`, registered in
`default_registry`. Gives app logic a wall-clock reading that survives replay.

**The rule (shared with [cap-scheduler](cap-scheduler.md) and
[cap-job-queue](cap-job-queue.md)):** wall-clock time is an effect. The core
never reads a clock inside `decide`/`fold`; the edge *observes* time and records
the observation as an event — a fact. Replay folds the recorded fact and never
consults a clock, exactly like `net.fetch` folds the recorded response and
never re-sends the request.

## What QuickJS exposes today (audited)

`terrane-cap-js-runtime/src/sandbox.rs` builds `rquickjs::Context::full`, which
installs every standard intrinsic. `install_app_globals` undefines only `eval`
and `Function`; **`Date` (and `Math.random`) are fully live** — `Date.now()`
inside a backend returns the real system clock, silently. `Instant::now` in the
sandbox is used only for the execution budget, not exposed to JS.

Under Option A this does **not** break replay-identity (replay never re-runs
JS; the run's `kv.*` writes are what fold). The hazard is subtler:

- the observation is invisible — no event, no grant, no audit trail;
- any future re-execution of backends (verification, divergence checks, an
  Option-B runtime) diverges immediately;
- harness/e2e tests over time-using apps are unmockable and flaky by
  construction.

v1 therefore ships the honest surface below and **leaves `Date` intact** with a
documented warning in `APP_API.md`; hardening the sandbox is a confirmable
decision, not a silent behavior change (see Decisions).

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `time.observe` | args `app` → `Decision::Effect(Effect::ObserveTime { app })` — **recorded** |
| Resource | `time.now()` | `ResourceMethod::Call` routing to `time.observe`; the edge reads `SystemTime`, emits `time.observed`, and `resource_call_output` returns the epoch-ms string to the backend (same recorded-call shape as `local-model.ask`) |
| Resource | `time.live()` | same effect wrapped in `Decision::TransientEffect` — live, **never recorded**; display-only timestamps (parallel to `net.get` vs `net.fetch`) |
| Resource | `time.last()` | `ResourceMethod::Read` — pure read of the app's last recorded observation from folded state, or null |
| Event | `time.observed` | payload (borsh) `{ app, epoch_ms: u64 }` |

Fold: `TimeState { last: BTreeMap<AppId, u64> }` — upsert on `time.observed`;
reacts to `app.removed` by dropping the app's entry. A capability-owned
`observed_event(app, epoch_ms)` constructor (the `fetched_event` pattern) keeps
the kind and payload shape inside the crate.

## Semantics

- **UTC epoch milliseconds only**, returned as a decimal string (resource
  returns are strings). No timezones, no formats — formatting and locale are
  the UI's job (`window.terrane` already owns locale).
- **No monotonicity guarantee.** Recorded values are facts about the host
  clock, which steps backward under NTP correction. Two `time.observed` events
  for one app may be non-increasing; consumers needing ordering should use the
  event-log order (which *is* total), not the timestamps. Documented in
  `doc.rs` so it's a choice, not an accident.
- `time.now()` inside one backend run may be called repeatedly; each call is
  one recorded event. Soft cap **32 recorded observations per run** (typed
  error naming `time.live()` as the escape hatch) so a loop can't bloat the
  log.

## Replay story

`time.observed` events fold into `TimeState`; the return value of a past
`now()` call is derivable from the log. Replay performs zero clock reads.
`time.live()` results never enter the log, so they can never be expected by
replay — the same contract as `net.get`.

## Security / permissions

Grant resource: `time` namespace-v1 (`call` + `read` methods), described as
"read the current wall-clock time (recorded)". Low sensitivity, but the grant
keeps the observation auditable and the permission prompt honest.

## Limits (in `doc.rs`, enforced in decide/edge)

- 32 recorded observations per backend run (transient `live()` uncapped).
- `epoch_ms` is `u64`; the edge errors (typed) if the clock reads pre-1970.

## Implementation plan

1. **Interface:** add `Effect::ObserveTime { app }` to
   `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-time`:** `lib.rs` (manifest, decide for
   `time.observe` + transient variant, fold, `resource_call_output`, reads,
   describe), `doc.rs`, `observed_event()` constructor.
3. **Edge:** `ObserveTime` arm in `EdgeRunner::run`
   (`terrane-host/src/edge.rs`): `SystemTime::now()` → `[time.observed]`.
4. **Register** in `default_registry`; `APP_API.md` documents
   `ctx.resource.time.now/live/last` and the `Date.now()` warning; scaffold
   recipe mention.
5. **Tests:** engine tests `terrane-core/tests/cap/time.rs` — decide/fold,
   replay identity, per-run cap, `app.removed`; e2e
   `terrane-host/tests/cap/time.rs` — JS backend calling `now()` twice, replay
   reproduces both values without a clock (pure, default-run).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Timers and wake-ups ([cap-scheduler](cap-scheduler.md)), monotonic/interval
clocks, timezone or calendar math, formatting, clock-skew handling across
replicas (each replica records its own facts), NTP awareness.

## Decisions to confirm

- **Monotonicity — recommendation: no guarantee, documented** (log order is
  the ordering primitive). Alternative: edge clamps each app's observation to
  `max(previous, now)` for per-app non-decreasing reads.
- **`Date` in the sandbox — recommendation: leave intact in v1 + document the
  hazard.** Alternatives: (a) undefine `Date.now` so apps must use
  `ctx.resource.time`; (b) pin `Date.now` at run start to the last recorded
  `time.observed` value (deterministic but stale and surprising).
- **Per-run recorded-observation cap — recommendation: 32.** Alternative:
  uncapped (accept log growth) or 1-per-run with memoized return.
- **Transient variant naming — recommendation: `time.live()`** to make
  "unrecorded" audible. Alternative: `time.nowTransient()` mirroring nothing
  we ship today.
