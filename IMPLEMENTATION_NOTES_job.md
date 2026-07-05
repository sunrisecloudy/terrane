# Implementation Notes: `job`

## Files changed

- Added `rust/crates/terrane-cap-job-queue/`
  - `Cargo.toml`
  - `src/lib.rs`
  - `src/doc.rs`
  - `tests/job_queue.rs`
- Added host worker and e2e coverage:
  - `rust/crates/terrane-host/src/job.rs`
  - `rust/crates/terrane-host/tests/cap/job_queue.rs`
- Added core coverage:
  - `rust/crates/terrane-core/tests/cap/job_queue.rs`
- Additive shared wiring:
  - `Cargo.toml`
  - `Cargo.lock`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-core/tests/cap/interface.rs`
  - `rust/crates/terrane-host/Cargo.toml`
  - `rust/crates/terrane-host/src/lib.rs`
  - `rust/crates/terrane-host/src/cli.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`
  - `docs/APP_API.md`

## Key design choices

- Implemented `terrane-cap-job-queue` as namespace `job`, with folded `JobState` owned by the capability. Replay folds recorded facts only; it never re-runs backend work.
- Host lifecycle commands `job.start`, `job.report`, and `job.reap` are trusted-host-only in core admission and refused through untrusted public capability dispatch.
- App-scoped commands `job.submit`, `job.cancel`, and `job.progress` are grant-gated in public authz using app arg 0.
- `scheduler tick` now drains due jobs after firing due schedules, matching the plan's shared host tick loop direction.
- The worker records reaps before starts, enforces per-host max 8 and per-app max 2 running attempts, starts due jobs, invokes `handle([verb, jobId, ...args])`, then records `job.completed` or `job.failed`.
- Retry policy is deterministic and recorded: host computes `next_attempt_at = finished_at + backoff(attempt)`; fold only applies the recorded timestamp.
- Stalls count toward `maxAttempts`; a stalled final attempt folds to terminal `failed`.
- Terminal states win: cancelled/done/failed jobs ignore later progress or terminal reports.
- Capability docs document durable workflow composition with `scheduler` rather than adding recurring-job semantics to `job`.

## Deviations and notes

- The generic runtime resource bridge cannot mint ids or read the host clock inside `ctx.resource.job.submit/progress`, so the app resource method accepts edge-supplied `jobId` and timestamp arguments. The CLI/host adapter mints ids and timestamps for `terrane job submit`; the durable core remains pure.
- No asynchronous daemon or cancel interrupt flag was added beyond the existing bounded synchronous `scheduler tick` drain path. This matches the current host scheduler shape in this worktree while preserving the recorded lifecycle contract.

## Tests proving properties

- `terrane-cap-job-queue/tests/job_queue.rs`
  - `retry_backoff_is_capped_exponential`
  - `lifecycle_folds_and_replays_identically`
  - `failed_attempt_requeues_when_retry_due_and_stall_counts_toward_attempts`
  - `cancellation_wins_over_late_terminal_events`
  - `validation_rejects_bad_inputs_and_retry_bounds`
  - `app_removed_drops_jobs`
- `terrane-core/tests/cap/job_queue.rs`
  - `job_resource_surface_is_registered`
  - `job_lifecycle_requires_host_for_edge_facts_and_replays`
  - `job_due_query_is_pure_over_folded_state`
  - `app_remove_clears_job_state`
- `terrane-host/tests/cap/job_queue.rs`
  - `job_submit_tick_complete_and_replay`
  - `failing_job_records_retry_backoff_and_terminal_failure`
- Existing inventory/contract tests updated and passing:
  - `interface::all_capability_docs_are_explicit_and_operational`
  - `interface::default_registry_exposes_registered_grant_resource_namespaces`
  - `public_command_inventory_covers_every_registered_command`
  - `public_query_inventory_covers_every_registered_query`
  - `grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `host::app_api_doc_resource_section_is_generated`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
