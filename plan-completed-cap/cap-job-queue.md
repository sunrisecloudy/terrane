# Capability: `job` — background jobs with retries and progress (DRAFT)

New crate `rust/crates/terrane-cap-job-queue/`, namespace `job`, registered in
`default_registry`. Long-running background work: submit an app backend verb,
have the host execute it off the request path, with retries, progress, and a
full audit trail. Builds **on** [cap-scheduler](cap-scheduler.md) (the host
clock loop drives retries and lease sweeps) and the js-runtime (attempts are
ordinary `js-runtime.run` executions). Time handling follows
[cap-time](cap-time.md).

**The rule (shared with cap-time and cap-scheduler):** the core never reads a
clock and never runs JS. Every lifecycle transition — submitted, started,
progress, completed, failed, stalled — is a fact the edge observed, recorded as
an event with the times the host saw. Replay folds the recorded attempt
history; it never re-runs a job (Option A: each attempt's own `kv.*` writes
were recorded by the run itself).

Queue state is **folded capability state** — no sidecar store, no host memory
of record. The daemon's only private state is the in-flight set; everything it
needs to resume after a crash is rebuilt by replay.

## Command / event / resource surface

Commands marked **host** require `CommandAuthority::TrustedHost`; apps cannot
forge lifecycle transitions.

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `job.submit` | args `app, job_id, verb, args_json, retry_json, submitted_at` → validate (verb token, caps, retry bounds) → `job.submitted`. `job_id` (ULID) and `submitted_at` are **edge-supplied facts**: the resource shim / host adapter generates them before dispatch — decide stays pure (no randomness, no clock) |
| Command | `job.cancel` | args `app, job_id, at` → `job.cancelled` unless already terminal |
| Command (host) | `job.start` | `app, job_id, attempt, started_at, lease_until` → `job.started` |
| Command (host) | `job.report` | terminal report: outcome `completed\|failed`, `output/error`, `finished_at`, computed `next_attempt_at` for retryable failures → `job.completed` or `job.failed` |
| Command (host) | `job.reap` | `app, job_id, at` — lease expired → `job.stalled` |
| Event | `job.submitted` | `{ app, job_id, verb, args, retry, submitted_at }` → status `queued` |
| Event | `job.started` | `{ app, job_id, attempt, started_at, lease_until }` → `running` |
| Event | `job.progress` | `{ app, job_id, attempt, pct, note, at }` → update progress; **renews lease** to `at + lease_ms` |
| Event | `job.completed` | `{ app, job_id, attempt, output, finished_at }` → terminal `done` |
| Event | `job.failed` | `{ app, job_id, attempt, error, finished_at, next_attempt_at? }` → `queued` (retry due at `next_attempt_at`) or terminal `failed` when attempts exhausted |
| Event | `job.stalled` | `{ app, job_id, attempt, at }` → attempt abandoned; re-`queued` immediately (counts as a failed attempt) |
| Event | `job.cancelled` | `{ app, job_id, at }` → terminal; later `completed/failed/progress` for that job fold as no-ops |
| Resource | `job.submit(verb, argsJson, retryJson?)` | returns `job_id`; `progress(jobId, pct, note)`; `cancel(jobId)` |
| Resource | `job.stat(jobId)` / `job.list(status?)` | pure state reads: status, attempt, progress, timestamps, last error |
| Host query | `job.due(now_ms)` | pure: queued jobs whose `next_attempt_at ≤ now_ms` + running jobs with `lease_until < now_ms` (to reap) — `now_ms` is an argument, never an ambient read |

Fold: `JobState { app → job_id → Job { verb, args, retry, status, attempt,
progress, timestamps, last_error, next_attempt_at, lease_until } }`; reacts to
`app.removed` by dropping the app's jobs. Events carry capability-owned
constructors (`started_event()` etc., the `fetched_event` pattern).

## Retry policy

```jsonc
{ "maxAttempts": 3, "baseDelayMs": 1000, "factor": 2, "maxDelayMs": 300000 }
```

Capped exponential backoff: attempt *n* retries after
`min(baseDelayMs * factor^(n-1), maxDelayMs)`. Bounds enforced in decide:
`maxAttempts ≤ 10`, `maxDelayMs ≤ 1h`. No jitter in v1 (single-host queue —
nothing to de-thunder; and `next_attempt_at` is recorded either way, so adding
jitter later is not a format change). Each retry is a **new attempt with its
own recorded events** — the full history replays without re-running anything.

## The worker (host edge daemon)

Shares the [cap-scheduler](cap-scheduler.md) tick loop in `terrane-host`:
long-running web/mac hosts run it natively; the CLI's `terrane scheduler tick`
also drains due jobs (bounded: starts what's due, waits for those attempts,
exits). Per tick:

1. reap: `job.due(now)` expired leases → dispatch `job.reap` (fact: stalled);
2. start: for each due queued job, within concurrency limits, dispatch
   `job.start`, then run `js-runtime.run <app> <verb> <job_id> [args…]` —
   the job id is passed as the first arg after the verb so the backend can call
   `ctx.resource.job.progress(jobId, pct, note)`;
3. report: on return, dispatch `job.report` with outcome and host-observed
   times; on retryable failure the host computes `next_attempt_at =
   finished_at + backoff(attempt)` and records it in `job.failed`.

**Crash mid-job:** the attempt holds a lease (`lease_until`, default 60s,
renewed by every `job.progress`). A crashed host never reports; the next tick
(any host) sees the expired lease, reaps it, and the job re-queues as a fresh
attempt. Recommendation: a stall **counts** toward `maxAttempts` (a crashing
job must not loop forever).

**Cancellation:** `job.cancelled` is terminal in fold. Queued → never starts.
Running → best-effort: the worker flags the in-flight run so the QuickJS
interrupt handler (already used for the budget) aborts it; if the attempt
completes anyway, its terminal event folds as a no-op. Effects the run already
performed (recorded `kv.*` writes) stand — cancellation is not rollback.

## Replay story

Replaying the log rebuilds every job's exact lifecycle — attempts, progress
trail, backoff times, stalls — from recorded facts alone: no clock, no JS
execution, no queue store. `job.due` is pure over that state, so a restored
home resumes retries where the log left off.

## Security / permissions

- `job.start/report/reap` require TrustedHost; `job.submit/progress/cancel`
  are app-scoped (an app touches only its own jobs — enforced in decide).
- Grant resource: `job` namespace-v1, described as "background execution of
  this app's own backend verbs with retries". No cross-app verbs, ever.
- `output`/`error`/`note` are truncated to caps below **before** the event is
  built, so oversized payloads never reach persistence.

## Limits (in `doc.rs`, enforced in decide / daemon)

- Per-app: ≤ **2** concurrent running attempts; ≤ 1 000 non-terminal jobs.
- Global: ≤ 8 concurrent attempts per host process.
- `args_json` ≤ 16 KiB; `output`/`error` ≤ 64 KiB (truncated, flagged);
  `note` ≤ 1 KiB; progress events ≤ 1/sec/job (daemon-side coalescing).
- Terminal jobs pruned from fold state? No — kept; log compaction is a
  platform-wide problem, not solved per-cap (multiuser-sync follow-up).

## Implementation plan

1. **Crate `terrane-cap-job-queue`:** retry-policy parse/validate + backoff
   fn (pure, table-tested); `lib.rs` (manifest, decide incl. TrustedHost
   checks + app-scoping, fold incl. terminal-wins/no-op rules, reads,
   `due(now_ms)` query, describe); `doc.rs`; event constructors.
2. **Host worker:** extend the scheduler daemon
   (`terrane-host/src/scheduler_daemon.rs`) with the reap/start/report loop,
   ULID + timestamp supply at the edge, concurrency gates, cancel-interrupt
   flag. Depends on cap-scheduler step 2.
3. **CLI:** `terrane job submit|stat|ls|cancel <app> …` thin adapters; tick
   integration.
4. **Register** in `default_registry`; `APP_API.md` documents
   `ctx.resource.job.*` and the invoked-verb contract
   (`handle([verb, jobId, ...args])`).
5. **Tests:** engine `terrane-core/tests/cap/job_queue.rs` — lifecycle folds,
   backoff table, `due(now_ms)` cases (retry due, lease expiry), stall counts
   toward attempts, cancel-then-complete no-op, authority + app-scoping
   rejections, replay identity, `app.removed`; e2e
   `terrane-host/tests/cap/job_queue.rs` — submit → run → complete round-trip,
   failing verb retries with recorded backoff, injected-clock reap, replay
   reproduces the whole history (pure, default-run; inject `now_ms`, no real
   sleeping).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Cross-replica/distributed workers (single home), priorities and fairness
beyond FIFO-by-due-time, job dependencies/DAGs, scheduled recurring jobs
(compose: a [cap-scheduler](cap-scheduler.md) cron whose verb calls
`job.submit`), streaming logs (progress notes only), rollback on cancel,
terminal-job pruning.

## Decisions to confirm

- **`job_id` origin — recommendation: edge-generated ULID passed into
  `job.submit`** (sortable, decide stays pure). Alternative: caller-supplied
  ids (dedup power, collision burden on apps).
- **Job-id plumbing to the backend — recommendation: argv convention
  `[verb, jobId, ...args]`.** Alternative: a `__terrane_job_id` global set by
  the runtime (needs a js-runtime change, cleaner surface).
- **Stalls count toward `maxAttempts` — recommendation: yes.** Alternative:
  unlimited stall-requeues with only real failures counting (crash-loop risk).
- **Lease — recommendation: 60s default, renewed by `job.progress`.**
  Alternative: separate `job.heartbeat` event (quiet jobs without progress
  need it; noisier log).
- **Backoff jitter — recommendation: none in v1** (recorded
  `next_attempt_at` makes it a non-breaking later addition). Alternative:
  ±20% jitter recorded per failure now.
- **Terminal completion after cancel — recommendation: fold as no-op,
  cancelled wins.** Alternative: record-but-mark (`done_after_cancel`) for
  auditability.
