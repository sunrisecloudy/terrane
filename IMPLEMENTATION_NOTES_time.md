# Implementation Notes — `time` capability

Implements `plan-completed-cap/cap-time.md` (recommended options for every
"Decision to confirm"; phased plan followed end to end).

## What it is

A compact capability that gives app backends a replay-safe wall-clock read.
Reading the clock is an **effect**, not a pure decide:

- `ctx.resource.time.now()` → `Decision::Effect(Effect::ObserveTime { app })`:
  the edge reads `SystemTime` once, emits a `time.observed { app, epoch_ms }`
  event, and `resource_call_output` returns the epoch-ms decimal string to the
  backend. Replay folds the recorded fact; it never consults a clock.
- `ctx.resource.time.live()` → `Decision::TransientEffect(Effect::ObserveTime)`:
  live, **unrecorded** sibling for display-only timestamps (parallel to
  `net.get` vs `net.fetch`). Result never enters the log.
- `ctx.resource.time.last()` → `ResourceMethod::Read`: pure read of the app's
  last recorded observation from folded `TimeState.last` (or null).

Fold: `TimeState { last: BTreeMap<AppId, u64> }` — upsert on `time.observed`;
reacts to `app.removed` by dropping the app entry. UTC epoch milliseconds only,
returned as decimal strings. **No monotonicity guarantee** (NTP can step the
clock back); event-log order is the ordering primitive — documented in `doc.rs`.

## Files changed

New crate `rust/crates/terrane-cap-time/`:
- `Cargo.toml`, `src/lib.rs` (manifest, decide, fold, describe, read_resource,
  resource_call_output, `recorded_call_per_run_limit`, `observed_event`,
  `system_time_to_epoch_ms`, `MAX_OBSERVATIONS_PER_RUN`), `src/doc.rs`,
  `src/tests.rs` (11 cap-level tests).

Interface (`rust/crates/terrane-cap-interface`):
- `src/abi.rs` — added `Effect::ObserveTime { app }`.
- `src/capability.rs` — added `pub struct RecordedCallCap { limit, escape_hint }`
  and default trait method `Capability::recorded_call_per_run_limit(method)` (default `None`).
- `src/lib.rs` — re-exported `RecordedCallCap`.

Core (`rust/crates/terrane-core`):
- `Cargo.toml` — added `terrane-cap-time` dep.
- `src/lib.rs` — added `TimeState` slice to `State` + `StateStore` get/get_mut
  arms; registered `TimeCapability` in `default_registry`; re-exported
  `RecordedCallCap`; added a per-run recorded-call counter field to
  `RuntimeResourceHost` and the small `enforce_recorded_call_per_run_limit`
  helper used in `call_resource`'s `Decision::Effect` arm.

Host (`rust/crates/terrane-host`):
- `Cargo.toml` — added `terrane-cap-time` dep.
- `src/edge.rs` — `Effect::ObserveTime` arm: `system_time_to_epoch_ms(SystemTime::now())`
  → `observed_event(app, ms)`. (Typed error if the clock reads pre-1970.)
- `src/public_authz.rs` — classified `time.now` as
  `GrantGated { namespace: "time", app_arg_index: 0 }` (app-scoped, low
  sensitivity; the plan's grant spec is `time` namespace-v1 `call`+`read`).

Tests:
- `rust/crates/terrane-core/tests/cap/time.rs` (engine) + registered in
  `main.rs`.
- `rust/crates/terrane-host/tests/cap/time.rs` (e2e, default-run — pure, no
  network/model) + registered in `main.rs`.
- `rust/crates/terrane-core/tests/cap/interface.rs` — added `"time"` to the
  `grant_resource_namespaces` golden list.
- `rust/crates/terrane-host/tests/public_authz.rs` — bumped the command/grant
  inventories for the one new command `time.now` and the new grantable
  namespace.

Docs:
- `docs/APP_API.md` — regenerated `ctx.resource` section (now includes
  `ctx.resource.time.now()/.live()/.last()`), via `UPDATE_DOCS=1`.

Shared files touched (for clean integration): root `Cargo.toml` (workspace
member + workspace dep), `rust/crates/terrane-core/Cargo.toml`,
`rust/crates/terrane-host/Cargo.toml`, `docs/APP_API.md`, the two `main.rs`
test entrypoints, and the two inventory tests above. All edits are additive
(one new line / one new arm each); unrelated lines were not reordered.

## Key design choice: per-run recorded-call cap (deviation from "decide/edge")

The plan's Limits section says the 32-recorded-observations-per-run cap is
"enforced in decide/edge". Under the current architecture that placement is
not cleanly achievable for a limit that must persist *across multiple decide
calls within one backend run*: `Capability::decide` is pure and read-only on
folded state, there is no run-boundary event the cap could subscribe to in
order to reset a per-run counter, and the edge `EffectRunner::run` is stateless
per call. Putting a `match namespace == "time"` in core would reintroduce the
central command/event coupling CLAUDE.md forbids.

So the cap is enforced one level up, **in `RuntimeResourceHost::call_resource`**
(core), through a new opt-in `Capability::recorded_call_per_run_limit(method) ->
Option<RecordedCallCap>`:
- The host is fresh per backend run (constructed in `run_runtime` from
  `self.state.clone()`), so a per-host counter is naturally per-run scoped — no
  reset hook needed.
- The counter is incremented only for recorded `Decision::Effect` calls;
  transient `Decision::TransientEffect` returns early and is never gated, so
  `time.live()` stays uncapped (the plan's escape hatch).
- The cap knowledge lives in the cap: `TimeCapability::recorded_call_per_run_limit`
  returns `Some(RecordedCallCap { limit: 32, escape_hint: "use ... time.live() ..." })`
  for `"now"` and `None` otherwise. Core calls the cap's own declaration; it
  hardcodes no `time`/`time.observed` string. The mechanism is generic and
  available to any future capability that needs it.
- Decide stays pure and replay-identity is preserved: the counter is ephemeral
  host state, never logged or folded; replay never runs `call_resource`.

When the cap is hit mid-run, the Option-A runtime fails the whole run
atomically (records commit only after a successful run), so the log can't be
bloated by a runaway loop — matching the plan's hazard, even better than
"32 committed then error".

## Other deviations / decisions

- **Command naming.** The plan's surface table names the recorded command
  `time.observe`. Resource routing (`call_resource`) builds the decided name as
  `"{namespace}.{method}"`, so the `time.now()` resource routes to `decide("time.now")`
  — there is no separate path to `time.observe`. To honour the user-facing JS
  surface `ctx.resource.time.now()` (which the plan emphasises), the recorded
  command is `time.now` and the **event** kind is `time.observed`. This mirrors
  `net` (`net.fetch`/`net.get`), is self-consistent, and is documented.
- **No CLI verb.** The plan adds no top-level `terrane time …` verb (step 4 only
  registers + documents the resource surface). `time.now` is reachable via
  `ctx.resource.time.now()` and via programmatic `Core::dispatch`; classified
  `GrantGated` so the public/MCP `capability_command` path can also use it.
  Not adding a CLI verb keeps the change compact.
- **Scaffold recipe not added.** The plan mentions a "scaffold recipe mention"
  in step 4. `time` is already a grantable namespace (now in
  `grant_resource_namespaces()`), so generated apps request it via their manifest
  `resources` list like any other. Adding a dedicated builder recipe in `mcp.rs`
  would over-build this compact cap, so it is intentionally omitted.
- **`Date.now()` left intact in v1** (plan recommendation); the hazard is
  documented as a constraint in `doc.rs` and in the cap doc, not a behavior
  change to the JS sandbox.

## Test names proving each property

Cap-level (`rust/crates/terrane-cap-time/src/tests.rs`):
- `observed_event_describes_and_folds_last_value` — event + fold + describe,
  non-monotonic upsert.
- `time_now_decides_recorded_effect_for_existing_app`,
  `time_live_decides_transient_effect` — decide → Effect / TransientEffect.
- `time_now_rejects_missing_app_before_any_effect` — AppNotFound before effect.
- `unknown_command_is_a_typed_error` — typed error on unknown verb.
- `last_returns_last_observation_or_null` — read resource returns value or null.
- `resource_call_output_returns_epoch_ms_string` — call output for `now`/`live`.
- `app_removed_drops_the_observation_entry` — `app.removed` subscription cleanup.
- `system_time_to_epoch_ms_handles_now_and_pre_epoch` — pre-1970 typed error.
- `recorded_call_cap_gates_now_only` — cap declared for `now`, not `live`/`last`.
- `doc_lists_surface_and_limits` — doc surface/limits/internal present.

Engine (`rust/crates/terrane-core/tests/cap/time.rs`):
- `time_now_resource_records_each_observation_and_replays` — two recorded
  observations, last folds to the second, replay-identity holds.
- `time_live_resource_returns_value_but_records_nothing` — live read returns a
  value, records nothing, state stays empty, replay ok.
- `time_now_per_run_cap_allows_32_then_a_fresh_run_allows_32_again` — cap is
  per-run (resets between runs, not a cumulative log budget).
- `time_now_per_run_cap_blocks_the_33rd_call` — 33rd recorded call rejected with
  a typed `InvalidInput` naming `time.live`; failed run commits nothing; replay ok.
- `time_now_requires_existing_app_before_any_effect` — missing app rejected in
  decide before any effect; existing app reaches the (NoEffects) runner.
- `time_observed_folds_last_value_without_a_clock` — pure fold of two recorded
  events + `app.removed` cleanup, no clock involved.

E2e (`rust/crates/terrane-host/tests/cap/time.rs`):
- `time_now_records_observation_and_replays_without_a_clock` — real `terrane`
  binary reads `SystemTime`, records two `time.observed`, `replay` ok.

## Gate (all green)

```
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help
```