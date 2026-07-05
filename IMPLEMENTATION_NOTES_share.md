# Share Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-share/` with deterministic share state, events, command/query handling, docs, and crate integration tests.
- Added core registration/wiring in `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, and `rust/crates/terrane-core/src/lib.rs`.
- Added `Effect::NewInviteToken` in `rust/crates/terrane-cap-interface/src/abi.rs` and host-edge token minting in `rust/crates/terrane-host/src/edge.rs`.
- Added host composition helpers in `rust/crates/terrane-host/src/share.rs` and exported the module from `rust/crates/terrane-host/src/lib.rs`.
- Added CLI verbs in `rust/crates/terrane-host/src/cli.rs`: `share invite`, `share redeem`, `share revoke`, `share ls`, and `share invites`.
- Tightened sync helper surface in `rust/crates/terrane-host/src/sync.rs` with explicit grantee-aware read/write checks and removed the old broad grant-on-pair behavior.
- Classified `share.*` public command/query inventory in `rust/crates/terrane-host/src/public_authz.rs` and `rust/crates/terrane-host/tests/public_authz.rs`.
- Added tests in `rust/crates/terrane-core/tests/cap/share.rs`, `rust/crates/terrane-host/tests/cap/share.rs`, and test module entries in each `main.rs`.

## Key design choices

- `share` owns only replayable facts: `share.invited`, `share.redeemed`, and `share.revoked`.
- Invite plaintext is generated at the host edge; the event log, queries, and `describe()` expose only the SHA-256 token hash or omit it.
- `write` implies read in `ShareState`; sync helper checks use `has_read` for outbound/pull and `has_write` for inbound/push.
- Pairing no longer grants app data by itself. Sharing is explicit via invite redemption.
- Host `share::redeem` composes `share.redeem` and `auth.grant`; `share::revoke` composes `share.revoke` and `auth.revoke`.
- Public `capability_command share.*` is explicitly refused because sharing is an owner/control-plane action.

## Deviations and notes

- The current `auth.grant` model only accepts namespaces with registered runtime resource grant specs; `sync` has no app-facing resource surface by design. The host mirror currently writes the visible permission record to the existing app data namespace `kv`, while actual sync enforcement uses folded `ShareState`.
- The current HTTP sync routes do not yet carry a bearer-token-to-peer identity into route helpers. This slice adds explicit `*_for_grantee` sync helpers and tests the rights table over those helpers; wiring HTTP bearer identity into the web route calls remains the natural edge follow-up.
- Invite TTL and failed-redemption burn limits are documented, but the deterministic capability has no clock and failed commands do not record events. Those policies belong in the host redeem edge once the route-level bearer/token flow is completed.

## Test proof

- `terrane-cap-share/tests/capability.rs`
  - `invite_describe_redacts_token_hash`
  - `app_removed_clears_share_state`
  - `validation_rejects_bad_rights_and_grantee`
- `terrane-core/tests/cap/share.rs`
  - `invite_redeem_revoke_are_replayable_and_hash_only`
  - `validates_inputs_and_clears_app_on_remove`
- `terrane-host/tests/cap/share.rs`
  - `share_cli_invite_redeem_revoke_mirrors_auth_and_replays`
  - `sync_share_rights_are_read_pull_only_and_write_bidirectional`
- `terrane-host/tests/public_authz.rs`
  - `public_command_inventory_covers_every_registered_command`
  - `public_query_inventory_covers_every_registered_query`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
