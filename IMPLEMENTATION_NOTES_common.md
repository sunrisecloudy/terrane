# Implementation Notes: common

## Files changed

- `rust/crates/terrane-cap-common/`: new capability crate for `common.send`, email schema validation, folded send state, docs, and integration tests.
- `rust/crates/terrane-cap-interface/src/abi.rs`: added `Effect::ChannelSend { app, channel, message }`.
- `rust/crates/terrane-core/src/lib.rs`: added `CommonState`, registered `CommonCapability`, and wired the state slice.
- `rust/crates/terrane-host/src/edge.rs`: added the `ChannelSend` edge arm, SMTP submit, MIME assembly, connection secret resolution, CAS attachment reads, and failed-send recording.
- `rust/crates/terrane-cap-auth/src/lib.rs`: taught `auth.grant` to parse `common:send:email` as a `common` resource id.
- `rust/crates/terrane-host/src/public_authz.rs`: explicitly refuses untrusted public `common.send`, because the generic namespace gate cannot represent the required channel-scoped grant.
- `docs/APP_API.md`: regenerated the `ctx.resource.common` section.
- Shared wiring: root `Cargo.toml`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-host/Cargo.toml`, `Cargo.lock`, and the relevant cap test `main.rs` files.

## Key design choices

- `common.send` is the outbound half only. Existing `common.receive` interop behavior was not reimplemented.
- Email is the first channel. The enforced send grant is `common:send:email`; a plain `common` namespace grant alone does not authorize sending.
- Decide-time validation is pure: channel schema, recipient syntax, body/subject/attachment limits, blob attachment metadata, channel grant, and folded rate limits are checked before returning `Effect::ChannelSend`.
- The recorded `common.sent` event stores recipients, subject, body hash, body recording mode, attachment blob metadata, status/error, connection name, and edge-observed `sent_at`; it does not store credentials.
- Failed sends still fold as `common.sent status=failed` and count as attempts.
- SMTP credentials are resolved only at the host edge from the connection secret store, after checking the app has `connection:<name>`.

## Deviations / notes

- The plan recommended `lettre`. This slice uses a small host-edge SMTP client over `TcpStream` instead, to avoid pulling a large new dependency tree while preserving the replay boundary and loopback SMTP proof. It supports plain SMTP with `AUTH PLAIN`; STARTTLS/provider APIs remain future edge work.
- Decide cannot read the wall clock. Rate-limit windows are computed from folded `sent_at` values; tests can pass optional `sentAt` in message JSON for deterministic limit proof. The edge records actual Unix seconds when omitted.
- `common.send` is refused through untrusted public `capability_command`; app backends and trusted host commands can use it, and decide still enforces the channel-scoped grant.

## Tests proving the properties

- Crate validation/docs: `terrane-cap-common/tests/capability.rs`
  - `email_message_canonicalizes_defaults_and_hash_only_body`
  - `record_body_true_inlines_small_body`
  - `validation_rejects_unknown_channel_and_bad_recipient`
  - `doc_lists_channel_limits`
- Engine/replay/security/limits: `rust/crates/terrane-core/tests/cap/common.rs`
  - `common_send_records_redacted_outcome_and_replays`
  - `missing_channel_grant_blocks_before_effect`
  - `email_rate_limit_counts_recorded_attempts`
  - `app_removed_clears_common_state`
- Host e2e SMTP/CAS/secret proof: `rust/crates/terrane-host/tests/cap/common.rs`
  - `common_send_email_uses_connection_and_blob_attachment_without_secret_log_leak`
- Inventory/docs/public surface:
  - `host::app_api_doc_resource_section_is_generated`
  - `interface::default_registry_exposes_registered_grant_resource_namespaces`
  - `public_command_inventory_covers_every_registered_command`
  - `public_query_inventory_covers_every_registered_query`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
