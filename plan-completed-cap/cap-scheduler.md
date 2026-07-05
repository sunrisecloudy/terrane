# Capability: `scheduler` — cron / delayed / recurring wake-ups (DRAFT)

New crate `rust/crates/terrane-cap-scheduler/`, namespace `scheduler`,
registered in `default_registry`. Lets an app say "run my backend verb at
epoch X / every N" without owning a process. Sibling of
[cap-time](cap-time.md); [cap-job-queue](cap-job-queue.md) builds on it.

**The rule (shared with cap-time):** the core never reads a clock. A timer
*firing* is a fact observed by the host — the host dispatches a recorded
`scheduler.fire` event carrying the times it saw. `decide` computes due-ness
only from arguments handed to it; `fold` only folds recorded facts. Replay
folds firings, it never re-derives them.

## Who owns the clock loop

The **host** does — the edge daemon lives in `terrane-host` and is driven by
whichever host process is long-lived:

- **web / mac hosts** (long-running): a tick loop (sleep until next-due, floor
  30s) queries due schedules from folded state and fires them. The natural
  home.
- **CLI host** (short-lived): no daemon. `terrane scheduler tick` computes and
  fires everything due *now*, then exits — suitable for launchd/cron to call.

Due-ness is a pure host query `scheduler.due(now_ms)` — `now_ms` is an
**argument** supplied by the caller, never an ambient read inside the
capability, so the same function is unit-testable and deterministic.

## Schedule spec

`scheduler.set` takes one canonical JSON spec (validated + canonicalized purely
in decide):

```jsonc
{
  "at":   1783728000000,          // one-shot, UTC epoch ms — XOR with "cron"
  "cron": "*/15 * * * *",         // standard 5-field cron (min hour dom mon dow), UTC
  "verb": "on_timer",             // backend verb to invoke on fire (default "timer")
  "args": ["daily-digest"]        // extra string args passed after the standard ones
}
```

Cron dialect: **standard 5-field, UTC, minute granularity** (the `cron` crate,
pinned, wrapped behind our own `next_after(spec, epoch_ms)` so it's swappable).
No seconds field, no `@reboot` sugar.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `scheduler.set` | args `app, name, spec_json` → validate, canonicalize → `Decision::Commit([scheduler.set])` |
| Command | `scheduler.clear` | args `app, name` → `scheduler.cleared` if present |
| Command | `scheduler.fire` | args `app, name, scheduled_for, fired_at, skipped` — **host-only** (`CommandAuthority::TrustedHost`); decide checks the schedule exists, then commits `scheduler.fired` verbatim (the times are host-observed facts) |
| Event | `scheduler.set` | `{ app, name, spec_json }` — upsert |
| Event | `scheduler.cleared` | `{ app, name }` — drop |
| Event | `scheduler.fired` | `{ app, name, scheduled_for, fired_at, skipped }` — update `last_*`, add `skipped` to `skipped_total`; a fired one-shot is dropped |
| Resource | `scheduler.set(name, specJson)` / `clear(name)` | app self-service, routed to the commands |
| Resource | `scheduler.list()` / `stat(name)` | pure state reads: spec + `last_scheduled_for/last_fired_at/skipped_total` |
| Host query | `scheduler.due(now_ms)` | pure: all `{app, name, scheduled_for, skipped}` due at `now_ms` given folded state |

Fold: `SchedulerState { app → name → Entry { spec, last_scheduled_for,
last_fired_at, skipped_total } }`; reacts to `app.removed` by dropping the
app's entries.

## Firing → running the backend

Fold is pure — it cannot execute JS. So a firing is **two host steps**:

1. dispatch `scheduler.fire` (recorded, TrustedHost) — the durable fact;
2. dispatch `js-runtime.run <app> <verb> <name> <scheduled_for> [args…]` — the
   normal backend path. Per Option A the run records only ordinary `kv.*`
   (etc.) events, so replay rebuilds its consequences without re-running JS.

If step 2 errors, the firing already happened (fact stands); the host logs the
run error. Retries/attempt tracking are deliberately [cap-job-queue]'s job —
apps needing retry semantics schedule a `job.submit` instead.

## Missed firings & overlap

- **Missed (host was off):** fire-once-on-catch-up. On tick, if one or more
  occurrences were missed since `last_scheduled_for`, fire a single
  `scheduler.fire` with `scheduled_for` = the most recent missed occurrence and
  `skipped` = the count of older missed occurrences. No replay storms.
- **Overlap:** skip-if-running. The daemon keeps an in-memory "in flight" set
  per `(app, name)`; a due schedule whose previous run hasn't returned is
  skipped this tick (retried next tick, no event — it wasn't a fact yet).
- One-shot in the past at `set` time: accepted, fires on next tick with
  `skipped: 0` (it's simply due).

## Replay story

Replay folds `set/cleared/fired` and reproduces `SchedulerState` exactly;
`scheduled_for`/`fired_at`/`skipped` come from the recorded payload, never a
clock. Backend consequences replay from the run's own recorded events. A
restored home resumes correctly because catch-up derives from folded
`last_scheduled_for`, which replay just rebuilt.

## Security / permissions

- `scheduler.fire` is rejected for non-`TrustedHost` callers — apps cannot
  forge firings for themselves or others.
- Grant resource: `scheduler` namespace-v1, described as "schedule backend
  wake-ups (cron / one-shot)". An app can only schedule **its own** verbs.

## Limits (in `doc.rs`, enforced in decide)

- ≤ **32** schedules per app; name ≤ 128 chars; spec ≤ 4 KiB.
- Cron minimum effective interval: **1 minute** (dialect-enforced).
- `verb` must be a plain token (no leading `__`), args ≤ 16 strings.

## Implementation plan

1. **Crate `terrane-cap-scheduler`:** spec parse/validate/canonicalize +
   `next_after` cron wrapper (pinned `cron` crate); `lib.rs` (manifest, decide
   incl. TrustedHost check on `fire`, fold, reads, `due(now_ms)` query,
   describe); `doc.rs`; `fired_event()` constructor.
2. **Host daemon:** `terrane-host/src/scheduler_daemon.rs` — tick loop
   (next-due sleep, 30s floor), in-flight overlap set, catch-up computation,
   fire → `js-runtime.run` follow-up. Wire into the web/mac host serve loops.
3. **CLI:** `terrane scheduler tick` + `terrane scheduler ls <app>` thin
   adapters.
4. **Register** in `default_registry`; `APP_API.md` documents
   `ctx.resource.scheduler.*` and the invoked-verb contract
   (`handle([verb, name, scheduledFor, ...args])`).
5. **Tests:** engine `terrane-core/tests/cap/scheduler.rs` — spec validation,
   `due(now_ms)` table cases (cron + one-shot + catch-up + skipped counts),
   fold/replay identity, authority rejection, `app.removed`; e2e
   `terrane-host/tests/cap/scheduler.rs` — `tick` fires a due one-shot, backend
   runs, replay reproduces state (pure, default-run; no sleeping — inject
   `now_ms`).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Sub-minute schedules, timezone-aware cron (UTC only), exactly-once across
replicas (each home fires independently; multi-replica coordination is a sync
design), retry/attempt semantics ([cap-job-queue](cap-job-queue.md)), pausing
schedules, firing UI apps (backends only).

## Decisions to confirm

- **Cron dialect — recommendation: standard 5-field UTC.** Alternatives:
  6-field with seconds; `@hourly` aliases (could add later, format-compatible).
- **Missed-firing policy — recommendation: fire-once-on-catch-up with
  `skipped` count.** Alternatives: fire every missed occurrence (replay-storm
  risk); skip all missed silently.
- **Overlap policy — recommendation: host-side skip-if-running, unrecorded.**
  Alternative: record a `scheduler.skipped` event per overlap (auditable but
  noisy).
- **Invocation contract — recommendation: spec-declared `verb` with standard
  prefix args `[verb, name, scheduled_for, ...args]`.** Alternative: fixed
  verb `"timer"` for all schedules.
- **Firing atomicity — recommendation: fire event first, run second, run
  errors logged only.** Alternative: record a follow-up `scheduler.run_failed`
  event for observability.
