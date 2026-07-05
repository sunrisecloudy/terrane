# MCP Client Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-mcp-client/` with the `mcp` capability, event/state types, command validation, docs, and integration tests.
- Wired the crate into `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`, and `rust/crates/terrane-host/Cargo.toml`.
- Extended shared ABI effects in `rust/crates/terrane-cap-interface/src/abi.rs` with `McpCall` and `McpTools`.
- Extended grant parsing/exact namespace helpers in `rust/crates/terrane-cap-auth/src/lib.rs`.
- Updated JS resource installation in `rust/crates/terrane-cap-js-runtime/src/sandbox.rs` so manifest resources such as `mcp:linear` install `ctx.resource.mcp`.
- Added host edge execution in `rust/crates/terrane-host/src/mcp_client.rs`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/cli.rs`, `rust/crates/terrane-host/src/lib.rs`, and `rust/crates/terrane-host/src/public_authz.rs`.
- Added core and host e2e tests in `rust/crates/terrane-core/tests/cap/mcp.rs` and `rust/crates/terrane-host/tests/cap/mcp.rs`.
- Updated capability inventory/public authz tests and generated app API docs in `docs/APP_API.md`.

## Key design choices

- `mcp.call` is a recorded effect at the edge. The decide phase validates app, connection, tool, args JSON, redaction, limits, and exact per-server grant, then returns `Effect::McpCall`; fold only applies `mcp.called` events.
- Replay identity is preserved by recording the external MCP result in the event log. Large JSON results are stored through blob CAS with a `blob.stored` event and an `mcp.called` reference/hash.
- Secrets stay outside the log. MCP connection transport JSON may contain `$secret` markers resolved by the host edge from the connection secret store immediately before the outbound call.
- Grants are exact per server: `mcp:<name>` is required. A broad `mcp` namespace grant does not authorize a specific server call.
- Public direct dispatch refuses connection administration and transient tool discovery; `mcp.call` is grant-gated before decide/edge execution.
- The runtime resource surface is `ctx.resource.mcp.call(connection, tool, argsJson)` and `ctx.resource.mcp.tools(connection)`, installed when an app manifest declares at least one `mcp:<name>` resource.

## Deviations from the plan

- Session reuse and idle teardown are not implemented in this slice. The host edge opens a transport per call, which keeps deterministic replay and security intact but is less efficient.
- Stdio MCP uses newline-delimited JSON-RPC for the implemented edge/test server path rather than the full Content-Length framed MCP transport.
- HTTP MCP uses simple JSON-RPC POST over `http`/`https` with SSRF checks and no streaming/SSE support.
- `mcp.tools` is implemented as transient host discovery and does not record folded state.

## Shared files touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-auth/src/lib.rs`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-cap-js-runtime/src/sandbox.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Test evidence

- `terrane-cap-mcp-client` integration tests:
  - `transport_validation_preserves_secret_markers_and_redacts_plain_sensitive_values`
  - `call_preparation_canonicalizes_keys_redacts_sensitive_args_and_hashes_unredacted_args`
  - `call_preparation_rejects_bad_shapes_and_limits`
- Core e2e:
  - `mcp_call_records_redacted_result_and_replays`
  - `mcp_call_requires_exact_server_grant`
  - `mcp_disconnect_and_app_removed_clean_folded_state`
- Host e2e:
  - `mcp_http_call_records_result_and_replays`
  - `mcp_http_tool_error_is_recorded_as_fact`
  - `mcp_call_without_exact_grant_is_denied_before_edge`
  - `mcp_http_large_result_offloads_to_blob`
  - `mcp_stdio_call_records_result`
- Inventory/doc/public authz coverage:
  - `interface::default_registry_exposes_registered_grant_resource_namespaces`
  - `interface::every_declared_resource_method_is_documented`
  - `public_authz::public_command_inventory_covers_every_registered_command`
  - `public_authz::grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `contract::surface_is_derived_from_the_live_declarations`

## Final validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
