# Automation Capability Implementation Notes

## Files Changed

- Added `rust/crates/terrane-cap-automation/`:
  - `src/lib.rs`, `commands.rs`, `events.rs`, `types.rs`, `resources.rs`, `matcher.rs`, `doc.rs`
  - `tests/capability.rs`
- Registered the crate in `Cargo.toml`, `Cargo.lock`, and `rust/crates/terrane-core/Cargo.toml`.
- Added `AutomationState` to `rust/crates/terrane-core/src/lib.rs`, registered `AutomationCapability` in `default_registry`, and marked `automation.fire` / `automation.suppress` trusted-host-only.
- Added `CapBus::event_kind_matches` in `rust/crates/terrane-cap-interface/src/runtime.rs` so rule validation checks declared event kinds without a central event enum.
- Added a small public KV event payload projection helper in `rust/crates/terrane-cap-kv/src/events.rs` / `lib.rs`, plus `serde_json` dependency, so the matcher can evaluate JMESPath over `kv.set` / `kv.deleted` payloads without centralizing all event decoding.
- Added host-edge matcher support in `rust/crates/terrane-host/src/automation.rs`.
- Added CLI parity in `rust/crates/terrane-host/src/cli.rs`: `terrane automation tick [--now-ms]` and `terrane automation ls <app>`.
- Updated public command/query auth inventory in `rust/crates/terrane-host/src/public_authz.rs` and tests.
- Updated capability tests:
  - `rust/crates/terrane-core/tests/cap/automation.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/cap/automation.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
- Regenerated `docs/APP_API.md` resource surface for `ctx.resource.automation.*`.

## Key Design Choices

- Rules are durable `automation.set` facts with canonical JSON and a SHA-256 `rule_hash`.
- Firings are host-owned `automation.fired` facts; suppressions are host-owned `automation.suppressed` facts. Replay only folds these facts and never runs the matcher.
- Filters reuse `terrane-cap-query`'s JMESPath wrapper over an envelope shaped as `{kind, actor, payload}`.
- Resource methods are app-scoped: `ctx.resource.automation.set/rm/list/stat`.
- Cross-app triggers require a normal existing grant on the source app for the source event namespace. For example, observing `kv.set` from `mailbox` requires a `kv` grant on `mailbox`.
- The matcher enforces the v1 budget of 8 firings per tick/process pass. Excess matches are recorded as `automation.suppressed`.
- The matcher currently projects KV event payloads (`kv.set`, `kv.deleted`) to JSON. Other event kinds can be added by capability-owned projection helpers without introducing a central event enum.

## Deviations / Limits

- The generic payload projection surface is intentionally not global in v1. Automation validates all declared event kinds, but only event kinds with a host-side JSON projection can be filter-matched today. This keeps capability-owned event decoding intact.
- The implemented host loop is exposed as `terrane_host::automation::{process_records, run_tick, run_tick_at}` and wired to CLI `automation tick`, matching the scheduler-style host helper pattern in this tree. Long-running hosts can call the same helper after committed records.
- No `actor` field is set by the capability; the engine stamps provenance at commit.

## Shared Files Touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/runtime.rs`
- `rust/crates/terrane-cap-kv/Cargo.toml`
- `rust/crates/terrane-cap-kv/src/events.rs`
- `rust/crates/terrane-cap-kv/src/lib.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`

## Tests Proving Properties

- Crate validation / replay / matcher:
  - `terrane-cap-automation::capability::set_fire_and_replay_rebuild_identical_state`
  - `terrane-cap-automation::capability::invalid_rule_is_rejected`
  - `terrane-cap-automation::capability::matcher_filters_kv_payloads`
- Core integration:
  - `automation::automation_resource_surface_is_registered`
  - `automation::automation_set_fire_suppress_and_replay`
  - `automation::automation_validation_rejects_unknown_event_bad_filter_and_cross_app_without_grant`
  - `automation::automation_matcher_honors_filter_cooldown_and_seen_refs`
- Host e2e:
  - `automation::automation_tick_fires_matching_kv_event_runs_backend_and_replays`
  - `automation::automation_tick_records_suppression_when_fire_budget_is_exhausted`
- Public/auth/doc inventories:
  - `public_command_inventory_covers_every_registered_command`
  - `grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `public_query_inventory_covers_every_registered_query`
  - `interface::default_registry_manifest_is_valid`
  - `interface::all_capability_docs_are_explicit_and_operational`
  - `host::app_api_doc_resource_section_is_generated`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
