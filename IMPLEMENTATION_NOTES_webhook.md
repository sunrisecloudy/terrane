# Webhook Capability Implementation Notes

## Files changed

- `rust/crates/terrane-cap-webhook/`: new deterministic capability crate.
- `rust/crates/terrane-cap-interface/src/abi.rs`: added `Effect::WebhookRegister`.
- `rust/crates/terrane-core/src/lib.rs`: added `WebhookState` to `State` and registered `WebhookCapability` in `default_registry`.
- `rust/crates/terrane-host/src/edge.rs`: host-side token minting for register/rotate effects.
- `rust/crates/terrane-host/src/lib.rs`: trusted host ingest helper and blob offload wiring.
- `rust/crates/terrane-host/src/cli.rs`: `webhook register`, `rotate`, `unregister`, and `ls`.
- `rust/crates/terrane-host/src/public_authz.rs`: grant-gated public webhook commands; `webhook.ingest` is trusted-host-only.
- `host/web/src/routes.rs`: unauthenticated inbound `POST /hook/<app>/<name>/<token>` listener, body/header/rate checks, and backend verb dispatch.
- `docs/APP_API.md`: webhook resource and app-facing delivery shape.
- `Cargo.toml`, `Cargo.lock`, `host/web/Cargo.toml`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-host/Cargo.toml`: additive workspace/dependency wiring.
- Tests:
  - `rust/crates/terrane-core/tests/cap/webhook.rs`
  - `rust/crates/terrane-host/tests/cap/webhook.rs`
  - `host/web/tests/web.rs`
  - updated capability/public-authz inventories.

## Key design choices

- The capability records inbound webhooks as replayable facts. Host edge code observes the HTTP request, builds the trusted ingest envelope, and commits `webhook.received`.
- `webhook.register` and `webhook.rotate` are edge effects. The host mints opaque 32-character hex tokens and records only the deterministic registration/rotation event.
- Ingest validates route identity, token, POST-only semantics, header limits, body limits, and route state before recording.
- Sensitive headers reuse `terrane-cap-net` redaction rules. Signature headers such as `x-hub-signature-256` are preserved.
- Bodies at or below 256 KiB are recorded inline. Larger bodies up to 32 MiB are recorded with a deterministic blob link name and then inserted into host CAS plus linked through the blob capability.
- `ctx.resource.webhook.list()` exposes registered routes without revealing tokens.
- No capability code sets `actor`; provenance remains engine-owned.

## Deviations and notes

- The implementation wires the web host listener and CLI. I did not find a separate macOS native HTTP listener surface in this worktree to wire; the core host helper is reusable by another host adapter.
- Large payload offload records the webhook delivery fact first, then the host stores/links the blob after the commit using the deterministic blob name from the delivery. This keeps replay identity for webhook state and uses the existing blob edge path for bytes.
- The web listener rate limit is host-local and intentionally not part of replayed capability state.

## Shared files touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `host/web/Cargo.toml`
- `host/web/src/routes.rs`

## Test evidence

- Core command happy path, redaction, typed errors, replay identity, and app cleanup:
  - `webhook_register_rotate_ingest_redacts_and_replays`
  - `webhook_validation_and_app_removed_are_replay_safe`
- Host CLI e2e:
  - `webhook_cli_register_rotate_and_list_routes`
- Web listener e2e including backend delivery, bad token, oversized body, and rate limit:
  - `webhook_loopback_post_records_event_and_invokes_backend`
- Public authorization inventory:
  - `public_authz_inventory_matches_command_surface`
  - `public_authz_allows_public_commands_and_refuses_effectful_or_unknown_commands`
- Capability interface/doc inventory:
  - `interface::all_registered_capabilities_have_descriptions`
  - `interface::describe_matches_registered_capabilities`

## Validation gate

All mandatory commands passed:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help
```
