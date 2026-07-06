# Presence Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-presence/` with the `presence` capability, docs, manifest, folded channel metadata, transient publish decision, and integration tests.
- Added `rust/crates/terrane-host/src/presence.rs` as the live in-process presence hub used by host effects and resource reads.
- Wired the capability into `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`, and `rust/crates/terrane-host/Cargo.toml`.
- Extended `rust/crates/terrane-cap-interface/src/abi.rs` with `Effect::PresencePublish`.
- Extended host edge handling in `rust/crates/terrane-host/src/edge.rs` and exported the host presence module from `rust/crates/terrane-host/src/lib.rs`.
- Added public authz entries in `rust/crates/terrane-host/src/public_authz.rs` and updated `rust/crates/terrane-host/tests/public_authz.rs`.
- Added JS/resource bridge support in `rust/crates/terrane-cap-js-runtime/src/sandbox.rs`, `host/web/src/routes.rs`, `host/web/src/js/app_shell.js`, and `host/web/src/js/terrane_shim.js`.
- Updated `docs/APP_API.md` for the generated resource surface.
- Added core and host e2e tests in `rust/crates/terrane-core/tests/cap/presence.rs` and `rust/crates/terrane-host/tests/cap/presence.rs`, plus module wiring.

## Key design choices

- Presence publish is deliberately transient. `presence.publish` decides to `Decision::TransientEffect(Effect::PresencePublish { .. })`, and host effect handling returns no event records.
- The folded presence state stores only durable channel metadata: channel name plus payload/rate limits. Live peer membership and delivered frames stay in the host hub and are never folded or replayed.
- Public apps use `ctx.resource.presence.publish(channel, json)` and `ctx.resource.presence.peers(channel)`. The top-level `presence.publish` command is not public-authz callable, which prevents accidental recording through normal command commit paths.
- The resource surface is grant-gated on the `presence` namespace and validates app existence, channel names, JSON payloads, payload byte limits, max channels per app, and per-peer publish rate limits.
- Host/web exposes a live-only HTTP bridge for app iframe publish calls plus `window.terrane.onPresence` and `window.terrane.publishPresence`. It does not synthesize a current value or replay historical presence messages.

## Deviations from the plan

- I did not add a durable event for publish payloads. This follows the locked task instruction that presence messages must never hit the log.
- Publish does not auto-create durable channel metadata. Channels are explicitly defined through `presence.channel.define`; publishing to an undefined channel uses default transient limits.
- The web surface implemented here is a process-local live bridge over the existing app shell route. I did not add a separate `/sync/presence` WebSocket endpoint in this slice, because the existing web host routing stack does not currently expose that upgrade path cleanly. The host hub is live-only and ready for a sync-v2 transport adapter without changing replay semantics.

## Shared files touched

- Workspace wiring: `Cargo.toml`, `Cargo.lock`
- Core registry/state: `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`
- ABI: `rust/crates/terrane-cap-interface/src/abi.rs`
- Host edge/authz: `rust/crates/terrane-host/Cargo.toml`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/lib.rs`, `rust/crates/terrane-host/src/public_authz.rs`
- Web host: `host/web/src/routes.rs`, `host/web/src/js/app_shell.js`, `host/web/src/js/terrane_shim.js`
- Generated docs: `docs/APP_API.md`

## Test proof

- Core capability happy path and replay identity:
  - `define_drop_and_replay_channel_metadata`
  - `resource_publish_records_nothing_and_replay_is_identical`
- Transient publish effect:
  - `publish_decides_transient_effect_only`
- Validation and typed errors:
  - `rejects_invalid_channel_and_payload_limits`
  - `rejects_too_many_channels`
- Public command safety:
  - `top_level_publish_is_refused_before_commit`
  - `public_authz_covers_every_command_with_expected_counts`
  - `dangerous_commands_are_refused`
- Host e2e transient behavior:
  - `presence_resource_publish_is_live_only_and_replay_safe`
  - `presence_resource_publish_enforces_rate_limit`
- Web bridge coverage:
  - `serves_catalog_ui_and_invoke_over_http`
- Generated API docs:
  - `app_api_doc_resource_section_is_generated`

## Validation commands

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
