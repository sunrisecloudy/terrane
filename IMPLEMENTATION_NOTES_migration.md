# Migration Capability Implementation Notes

## Files changed

- New capability crate: `rust/crates/terrane-cap-migration/`
- Runtime support: `rust/crates/terrane-cap-js-runtime/src/{bundle.rs,lib.rs,sandbox.rs}`
- Core wiring: `rust/crates/terrane-core/{Cargo.toml,src/lib.rs,tests/cap/main.rs,tests/cap/migration.rs}`
- Host/CLI wiring: `rust/crates/terrane-host/{Cargo.toml,src/cli.rs,src/lib.rs,src/public_authz.rs,tests/cap/main.rs,tests/cap/migration.rs,tests/public_authz.rs}`
- Shared runtime host API: `rust/crates/terrane-cap-interface/src/runtime.rs`
- Workspace wiring: `Cargo.toml`, `Cargo.lock`
- Docs: `docs/APP_API.md`

## Key design choices

- `migration` is a normal capability crate with namespace `migration`, commands `migration.apply` and `migration.commit`, query `migration.status`, and event `migration.applied`.
- App data defaults to version 1. Folded migration state stores `app -> { version, history }` and drops that fact on `app.removed`.
- `migration.apply` validates app existence, script size, and consecutive `to = current + 1`, computes the script SHA-256, and returns `Decision::Runtime`.
- Migration scripts run through the existing QuickJS sandbox with a migration entrypoint `migrate(ctx)`. The runtime installs the app's declared resources, but refuses effectful namespaces such as net/model/browser/time/geo/MCP.
- The final version event is appended by calling runtime-internal `migration.commit` through the runtime host, so ordinary data writes and `migration.applied` land in one commit batch.
- Core gates `js-runtime.run` against manifest `dataVersion` before running a backend. Older folded data gets a typed refusal naming `terrane migrate <app>`; newer folded data refuses older code.
- `terrane migrate <app>` walks manifest steps one at a time. A failing later step leaves earlier steps committed and rerunnable/resumable.
- No capability code sets `actor`; core still stamps actor at commit.

## Deviations from plan

- The stale/newer backend gate is implemented in core's `run_runtime` path for `js-runtime.run` rather than only in CLI/host helper code. This keeps all js-runtime callers consistent while preserving deterministic replay: the gate only reads folded state and the app bundle manifest already read by the runtime path.
- `migration.status` is not allowed through untrusted public `capability_query` by default; trusted CLI/core query paths can still use it.

## Shared files touched for integration

- `Cargo.toml` and `Cargo.lock`: workspace member/dependency wiring.
- `rust/crates/terrane-core/src/lib.rs`: `MigrationState`, `StateStore`, `default_registry`, trusted-host gate, js-runtime data-version gate.
- `rust/crates/terrane-host/src/public_authz.rs` and `rust/crates/terrane-host/tests/public_authz.rs`: explicit refusal/inventory for migration commands and query.
- `rust/crates/terrane-cap-interface/src/runtime.rs`: non-consuming `record_count()` for enforcing the per-step recorded-event limit without draining the commit batch.

## Proof tests

- `migration::migration_apply_records_data_and_version_in_one_replayable_batch`
- `migration::throwing_migration_commits_no_data_or_version`
- `migration::migration_apply_refuses_public_gap_and_downgrade_paths`
- `migration::migration_state_drops_on_app_removed`
- `migration::migration_e2e_gates_stale_run_then_applies_manifest_step`
- `migration::migration_e2e_resumes_after_mid_sequence_failure`
- `interface::all_capability_docs_are_explicit_and_operational`
- `public_authz::public_command_inventory_covers_every_registered_command`
- `public_authz::public_query_inventory_covers_every_registered_query`

## Validation run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
