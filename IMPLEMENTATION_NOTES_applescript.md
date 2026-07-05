# AppleScript Capability Implementation Notes

## Files Changed

- Added `rust/crates/terrane-cap-applescript/`:
  - `Cargo.toml`
  - `src/lib.rs`
  - `src/events.rs`
  - `src/types.rs`
  - `src/doc.rs`
  - `tests/capability.rs`
- Added engine tests:
  - `rust/crates/terrane-core/tests/cap/applescript.rs`
- Added host edge and e2e tests:
  - `rust/crates/terrane-host/src/applescript.rs`
  - `rust/crates/terrane-host/tests/cap/applescript.rs`

## Shared Files Touched

- Workspace/dependency wiring:
  - `Cargo.toml`
  - `Cargo.lock`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-host/Cargo.toml`
- Core surface:
  - `rust/crates/terrane-cap-interface/src/abi.rs`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-core/tests/cap/interface.rs`
- Host surface:
  - `rust/crates/terrane-host/src/lib.rs`
  - `rust/crates/terrane-host/src/edge.rs`
  - `rust/crates/terrane-host/src/cli.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`
- Docs:
  - `docs/APP_API.md`

## Key Design Choices

- Extracted the complete branch capability rather than rebuilding it.
- Kept `applescript.run` and `applescript.check` as deterministic decide-time validation plus `Decision::Effect`.
- Added `Effect::AppleScriptRun` and `Effect::AppleScriptCheck`; `terrane-host` performs `/usr/bin/osascript` and `/usr/bin/osacompile` at the edge and records `applescript.ran` / `applescript.checked`.
- Replay folds recorded AppleScript facts only; it never spawns the macOS tools.
- Preserved the full script in the recorded event payload for auditability, bounded by `MAX_SCRIPT_BYTES = 64 KiB`.
- Added `AppleScriptState` to core state for per-app run history, deterministically truncated to `MAX_RUNS_PER_APP = 100`.
- Added `applescript` to default registry, grant resources, public authz, CLI help/state output, and generated app API resource docs.
- Left actor stamping to the engine. Capability-created events use `encode_event`, whose empty actor field is overwritten on commit.

## Deviations From Plan

- The plan recommends an optional `applescript.supports` probe. I did not add it because the extracted branch surface did not include it, and the task was extraction/adaptation rather than new surface design.
- The plan mentions script preview in elicitation as a future/phase item. I did not alter web/mac shell elicitation UX in this extraction pass; public authz now grant-gates the commands, and docs call out the danger.
- I fixed the extracted host temp filename for `osacompile` to use system time rather than `Instant::now().elapsed()`, avoiding avoidable collisions while preserving stdin-based compile checks.

## Proof Tests

- Happy path / decide effects:
  - `terrane-cap-applescript tests/capability.rs::applescript_run_returns_effect_and_folds_recorded_run`
  - `terrane-cap-applescript tests/capability.rs::applescript_check_returns_effect`
- Input validation / typed errors:
  - `terrane-cap-applescript tests/capability.rs::applescript_rejects_missing_apps_empty_and_oversize_scripts`
  - `terrane-host tests/cap/applescript.rs::applescript_rejects_unknown_app_and_empty_script`
- Replay identity:
  - `terrane-core tests/cap/applescript.rs::applescript_run_replays_identically_with_stub_runner`
- Replay folds recorded facts without spawning:
  - `terrane-core tests/cap/applescript.rs::ran_event_folds_recorded_run_without_spawn`
- Deterministic truncation and cleanup:
  - `terrane-cap-applescript tests/capability.rs::applescript_truncates_runs_deterministically_and_cleans_removed_apps`
- Host effectful e2e:
  - `terrane-host tests/cap/applescript.rs::applescript_run_e2e_real` is `#[ignore = "runs real osascript"]`
- Authz/inventory:
  - `terrane-host tests/public_authz.rs::applescript_commands_are_grant_gated_for_public_callers`
  - `terrane-host tests/public_authz.rs::public_command_inventory_covers_every_registered_command`
  - `terrane-core tests/cap/interface.rs::default_registry_exposes_registered_grant_resource_namespaces`
- Docs/resource inventory:
  - `terrane-core tests/cap/host.rs::app_api_doc_resource_section_is_generated`
  - `terrane-core tests/cap/interface.rs::every_declared_resource_method_is_documented`

## Validation Run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
