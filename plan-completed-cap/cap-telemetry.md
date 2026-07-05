# Capability: `telemetry` — app logging + error reporting

New crate `rust/crates/terrane-cap-telemetry/`, namespace `telemetry`,
registered in `default_registry`. Gives apps structured logging and gives
agents a way to read it — today there is **no `console` object at all** in the
QuickJS sandbox (`terrane-cap-js-runtime/src/sandbox.rs` installs only
`ctx.resource.*`; `console.log` throws `ReferenceError`), and a backend
exception surfaces only as one `Error::Runtime(CaughtError.to_string())`
returned to the caller — nothing is kept anywhere an agent can fetch later.
The headline use case: an agent builds an app over MCP, runs it, and pulls its
logs to self-debug.

## Locked decision

**Logs do not enter the event log.** Log lines go to a host-side per-app ring
buffer — rotating jsonl under `$TERRANE_HOME/logs/<app>/` — and only
error-class entries are (optionally) recorded as `telemetry.error` events.
This is a deliberately transient surface, and it is *compatible with* the one
rule, not an exception to it: the rule says replaying the event log must
reproduce identical **State**, and debug chatter folds into no state — it is
an observation of a run, like the `sysinfo` live reads in
`terrane-host/src/metrics.rs` or the `crypto` session keyring, both already
outside the log by design. Crash *facts* are different: "this app errored,
with this message" has replay value (error counters, last-error state an admin
console can fold), so `telemetry.error` is a real event. Recording
`debug`-level chatter would bloat the log that sync ships and replay walks,
for zero folded state — that is why the line is drawn exactly here.

## App surface

JS (declared in `manifest.resources` as `telemetry`; the sandbox also installs
a global `console` shim mapping `log/info→info`, `warn→warn`,
`error→error`, `debug→debug` so ordinary JS just works):

| Method | Semantics |
| --- | --- |
| `ctx.resource.telemetry.debug(msg, dataJson?)` | ring buffer only |
| `ctx.resource.telemetry.info(msg, dataJson?)` | ring buffer only |
| `ctx.resource.telemetry.warn(msg, dataJson?)` | ring buffer only |
| `ctx.resource.telemetry.error(msg, dataJson?)` | ring buffer **+** `telemetry.error` event |

Mechanics: `debug/info/warn` route as `Decision::TransientEffect(Effect::AppLog
{ app, level, msg, data })` — executed at the edge, result returned, **never
recorded** (the existing `TransientEffect` contract in
`terrane-cap-interface::abi`). `error` is an ordinary
`Decision::Effect` whose runner appends to the buffer *and* returns the
`telemetry.error` event.

### Auto-capture (js-runtime)

- A backend exception (the `caught_to_err` path in `sandbox.rs`) is mirrored
  to the ring buffer with the full `CaughtError` rendering — message **and
  stack** — plus the verb/input that triggered it, and emits one
  `telemetry.error { source: "exception" }` event alongside the existing error
  return. Same for the budget-interrupt timeout and the resource `first_error`
  slot.
- The `console` shim is installed whether or not `telemetry` is granted;
  without the grant it writes the ring buffer only via the host (never
  events), so logging never triggers a permission prompt — reading does.

## Event surface

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `telemetry.error` | `{ app, source, message, stack, data_digest }` | per-app `error_count`, ring of last 20 `ErrorFact`s in `TelemetryState` |
| (reacts) `app.removed` | — | drop the app's slice (edge also deletes `logs/<app>/`) |

`message`/`stack` are truncated (below); `data_digest` is sha256 of the
`dataJson` — the payload itself stays in the jsonl, keeping events small.

## Ring buffer layout

- `$TERRANE_HOME/logs/<app>/current.jsonl`, rotated at 4 MiB into
  `1.jsonl…3.jsonl` (≈16 MiB/app hard ceiling, oldest dropped).
- One line per entry: `{ts, level, msg, data, verb?, source?}` — `ts` is
  edge wall-clock (fine: nothing folds from it).
- Written only by the host edge; the core never opens it (same stance as
  `blobs.sqlite3` in [cap-blob.md](cap-blob.md)).

## Retrieval

| Surface | Shape |
| --- | --- |
| CLI | `terrane logs <app> [--level warn] [--tail 200] [--follow]` |
| MCP | new `app_logs { app, level?, tail? }` tool — the agent self-debug loop: build → invoke → `app_logs` → fix |
| Shell | dev panel in web/mac hosts: `GET /apps/<id>/logs?level=&tail=` (owner-only, same auth as invoke), rendered as a collapsible console under the app frame |
| App | `ctx.resource.telemetry.read(level?, tail?)` — an app may read *its own* buffer (grant-gated `read`) |

**No network export, ever:** logs never leave the machine except through the
local host routes above — one line in `doc.rs`, load-bearing for privacy.

## Replay story

Replay reproduces `TelemetryState` (error counts + last error facts) from
`telemetry.error` events alone; the jsonl files are non-authoritative
artifacts a fresh replica simply doesn't have — by design, like blob bytes
minus the hash check. Nothing in fold ever reads the filesystem.

## Security / permissions

- Grant resource `telemetry` namespace-v1: `call` (write levels) + `read`
  (own logs), described as "structured app logging and reading back this
  app's own log buffer". Cross-app log reads are host/owner surfaces only.
- `dataJson` may contain user data; that is why it stays in the local jsonl
  and only a digest enters the (syncable) event log.

## Limits (in decide / edge, typed errors)

- `msg` ≤ 8 KiB, `dataJson` ≤ 32 KiB (truncated with a marker, not errored —
  a log call should not crash the app), `stack` ≤ 16 KiB in events.
- Rate: ≤ 1 000 entries per backend run (excess dropped with one final
  "N entries dropped" line); `telemetry.error` *events* additionally deduped
  at the edge per identical `(app, message)` within a run — the buffer keeps
  every occurrence, the log keeps the fact.

## Implementation plan

1. **Interface:** `Effect::AppLog { app, level, msg, data }`.
2. **Crate `terrane-cap-telemetry`:** manifest (resources, `telemetry.error`,
   `app.removed` subscription, grant resource), decide (level routing,
   truncation), `error_event()` constructor, fold, describe, `doc.rs`.
3. **Edge sink:** `terrane-host/src/app_log.rs` — open/rotate jsonl, append,
   tail-read, delete-on-remove; wire `Effect::AppLog` (transient + recorded
   arms) into `EdgeRunner::run`.
4. **js-runtime:** install the `console` shim; mirror `caught_to_err`,
   interrupt-timeout, and `first_error` into the sink with stacks.
5. **Retrieval:** CLI `terrane logs`; MCP `app_logs` tool (+ mention in the
   build-flow `nextToolCall` hints so agents discover it after a failed
   invoke); web/mac dev-panel route.
6. **Docs:** `APP_API.md` (`ctx.resource.telemetry.*`, the `console` shim,
   the not-in-the-event-log stance).
7. **Tests:** engine tests `terrane-core/tests/cap/telemetry.rs` (error fold,
   truncation, replay identity, app.removed); e2e
   `terrane-host/tests/cap/telemetry.rs` (backend `console.log` → jsonl,
   thrown exception → stack in buffer + `telemetry.error` event, rotation at
   cap, `terrane logs` tail, MCP `app_logs`) — default-run, no network.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Network/remote export (never, not just v1), log levels configurable per app,
structured tracing spans, UI-side (`window.terrane`) log capture (the browser
console already exists there; a shell forwarder is a later nicety), metrics
counters/gauges (sysinfo covers machine metrics; app metrics are a separate
need), and log-based alerting.

## Decisions to confirm

- **Resource spelling** — *recommendation:* namespace `telemetry`, reached as
  `ctx.resource.telemetry.*`, plus the global `console` shim (zero-friction
  path agents already emit). *Alternative:* alias it to `ctx.resource.log.*`
  in the sandbox — reads better but adds the first namespace≠surface alias to
  `install_resources`; only worth it if the shim proves insufficient.
- **`telemetry.error` recording default** — *recommendation:* on by default
  (crash facts are worth their bytes), edge-deduped per run. *Alternative:*
  opt-in per app manifest flag — quieter logs, but agents lose the replayable
  error trail precisely when unattended.
- **UI log forwarding** — *recommendation:* defer; backend + exceptions cover
  the self-debug loop. *Alternative:* `window.terrane.log()` forwarding
  through the bridge into the same buffer — do it when a real UI-debug need
  lands.
