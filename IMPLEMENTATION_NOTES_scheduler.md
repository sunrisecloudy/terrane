# Scheduler Implementation Notes

## Files changed

- `rust/crates/terrane-cap-scheduler/src/*`: replaced the prior `create/pause/resume/remove/run.*` partial surface with the planned `set/clear/fire` surface, canonical schedule specs, folded fire state, pure due calculation, resources, docs, and integration tests.
- `rust/crates/terrane-cap-scheduler/tests/scheduler.rs`: moved scheduler tests out of `src/` and covered validation, replay identity, catch-up, one-shot removal, and `app.removed`.
- `rust/crates/terrane-core/src/lib.rs`: gated `scheduler.fire` as trusted-host-only at the core admit layer.
- `rust/crates/terrane-core/tests/cap/scheduler.rs`: updated core capability tests for the final scheduler surface and public authority rejection.
- `rust/crates/terrane-host/src/scheduler.rs`: changed host tick execution to record `scheduler.fire` first, then invoke the backend as `[verb, name, scheduled_for, ...args]`.
- `rust/crates/terrane-host/src/cli.rs`: added `terrane scheduler tick [--now-ms <epoch-ms>]` and `terrane scheduler ls <app>`.
- `rust/crates/terrane-host/src/public_authz.rs` and `rust/crates/terrane-host/tests/public_authz.rs`: classified `scheduler.set`/`clear` as grant-gated, `scheduler.fire` as refused for public callers, and kept `scheduler.due` unclassified/refused for public query callers.
- `rust/crates/terrane-host/tests/scheduler.rs` and `rust/crates/terrane-host/tests/cap/scheduler.rs`: added host tick tests and real binary e2e coverage.
- `docs/APP_API.md` and `docs/scheduler-premium-proof-plan.md`: updated resource tables and Premium proof guidance to the final scheduler contract.

## Key design choices

- The core never reads a clock. `scheduler.due(now_ms)` and `schedules_due_at(state, now_ms)` are pure over folded state and supplied arguments.
- `scheduler.fire` records host-observed facts verbatim: `scheduled_for`, `fired_at`, and `skipped`.
- Replay only folds `scheduler.set`, `scheduler.cleared`, and `scheduler.fired`; one-shot schedules are dropped by folding the fire.
- Recurring catch-up emits one due item for the newest missed occurrence and reports older missed occurrences in `skipped`.
- Schedule spec JSON is canonicalized with `BTreeMap` so event payload order is stable under workspace feature unification.
- The host tick records the fire before invoking the backend. Backend failures are returned/loggable at the host edge and do not mutate scheduler state.

## Deviations from the plan

- The implementation uses an internal deterministic five-field UTC cron matcher instead of adding the external `cron` crate. This avoids lockfile/dependency churn in this worktree while keeping the public `next_after` wrapper and standard minute-granularity dialect the plan requires.
- The long-running web/mac sleep loop described as a host daemon is not introduced as a background thread in this slice; the reusable host tick helper and CLI tick path are complete and can be driven by long-running hosts or external schedulers.

## Shared files touched

- `docs/APP_API.md`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/scheduler.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/src/scheduler.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `rust/crates/terrane-host/tests/scheduler.rs`

## Proof tests

- `terrane-cap-scheduler tests/scheduler.rs`
  - `set_fire_and_replay_rebuild_identical_state`
  - `one_shot_due_once_and_is_dropped_after_fire`
  - `catch_up_collapses_missed_cron_occurrences`
  - `invalid_spec_and_limits_are_rejected`
  - `app_removed_drops_schedules`
- `terrane-core tests/cap/scheduler.rs`
  - `scheduler_resource_surface_is_registered`
  - `scheduler_public_state_replays_and_cleanup_on_app_remove`
- `terrane-host tests/scheduler.rs`
  - `scheduler_due_loop_invokes_backend_after_recording_fire`
  - `scheduler_due_loop_keeps_fire_fact_when_backend_fails`
- `terrane-host tests/cap/scheduler.rs`
  - `scheduler_tick_fires_due_one_shot_runs_backend_and_replays`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
