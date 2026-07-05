# Telemetry Capability Implementation Notes

## Files changed

- New capability crate: `rust/crates/terrane-cap-telemetry/`
- Workspace wiring: `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-host/Cargo.toml`, `rust/crates/terrane-cap-js-runtime/Cargo.toml`
- Core wiring: `rust/crates/terrane-core/src/lib.rs`
- Interface/runtime hook: `rust/crates/terrane-cap-interface/src/abi.rs`, `rust/crates/terrane-cap-interface/src/runtime.rs`
- JS runtime: `rust/crates/terrane-cap-js-runtime/src/sandbox.rs`
- Host edge/CLI/MCP/web: `rust/crates/terrane-host/src/app_log.rs`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/cli.rs`, `rust/crates/terrane-host/src/lib.rs`, `rust/crates/terrane-host/src/mcp.rs`, `host/web/src/routes.rs`
- Public surface/contract/authz: `rust/crates/terrane-api/src/lib.rs`, `rust/crates/terrane-api/tests/contract.rs`, `rust/crates/terrane-host/src/public_authz.rs`
- Tests: `rust/crates/terrane-core/tests/cap/telemetry.rs`, `rust/crates/terrane-core/tests/cap/main.rs`, `rust/crates/terrane-host/tests/cap/telemetry.rs`, `rust/crates/terrane-host/tests/cap/main.rs`, `rust/crates/terrane-host/src/mcp_tests.rs`, `rust/crates/terrane-core/tests/cap/interface.rs`, `rust/crates/terrane-host/tests/public_authz.rs`
- Docs: `docs/APP_API.md`

## Key design choices

- `telemetry` is a normal capability crate registered in `default_registry` with `TelemetryState` in core `State`.
- Debug/info/warn log calls decide to `Decision::TransientEffect(Effect::AppLog { ... })`; they append to the host buffer and record no events.
- Error log calls decide to `Decision::Effect(Effect::AppLog { ... })`; the edge appends to the buffer and returns a `telemetry.error` event.
- Replay identity is preserved because only `telemetry.error` events fold into `TelemetryState`; jsonl buffers are host-edge artifacts only.
- The JS sandbox always installs `console`; it writes through a runtime-host telemetry hook so logs work even when `ctx.resource.telemetry` is absent.
- Runtime exceptions and first resource errors are mirrored to the log buffer. If telemetry is granted, failed runtime runs commit only captured `telemetry.error` facts, not partial app writes.
- Retrieval surfaces are local only: `terrane logs`, MCP `app_logs`, `GET /apps/{id}/logs`, and `ctx.resource.telemetry.read`.

## Deviations

- Per-run dedup of identical `(app, message)` error events is not separately implemented. The existing `recorded_call_per_run_limit` caps recorded telemetry errors at 1,000 per backend run, and the buffer still keeps occurrences as required.

## Test evidence

- Engine: `telemetry_error_records_state_and_replays`, `telemetry_debug_is_transient_and_top_level_rejected`, `telemetry_truncates_message_and_data_digest_uses_truncated_data`, `telemetry_rejects_invalid_error_source`, `telemetry_app_removed_drops_app_slice`
- Host e2e: `console_log_writes_local_buffer_and_cli_tails_it_without_grant`, `thrown_exception_writes_buffer_and_records_error_fact_when_granted`, `app_remove_prunes_log_buffer_directory`, `app_log_rotates_at_cap`
- MCP: `app_logs_tool_reads_local_app_buffer`
- Contract/web/authz inventory: `mcp_tool_surface_is_the_documented_set_with_valid_schemas`, `host_contract_lists_the_v1_subset`, `web_host_serves_every_declared_route`, `default_registry_exposes_registered_grant_resource_namespaces`, `grantable_command_inventory_requires_explicit_extractors_or_refusal`

## Gate

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
