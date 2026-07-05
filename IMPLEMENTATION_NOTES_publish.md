# Publish capability implementation notes

## Files changed

- Added `rust/crates/terrane-cap-publish/` with the deterministic capability state, event validators, structured docs, and integration tests.
- Added `Effect::InstallSignedBundle` in `rust/crates/terrane-cap-interface/src/abi.rs`.
- Registered `terrane-cap-publish` in the workspace, `terrane-core`, default registry, typed state store, and core cap tests.
- Added host edge signing/install support in `rust/crates/terrane-host/src/publish.rs`, with shared bundle helpers exposed from `src/edge.rs`.
- Wired CLI `app export` and signed `app install`, preserving legacy local directory installs for development bundles.
- Added MCP/API `app_install`, host public authorization refusal, and contract/authz test inventory updates.
- Added host e2e tests in `rust/crates/terrane-host/tests/cap/publish.rs`.
- Updated the existing loopback net test helper to read full HTTP request bodies before asserting, avoiding split-header/body TCP failures in the required workspace gate.

## Design choices

- The core capability records replayable public facts only: publisher trust, installed provenance, and identity metadata. It never stores private key material and never sets `actor`.
- Host export signs with the durable person key through the existing person/connection secret-store path. The private key remains at the edge; the archive carries only public key and signature material.
- Signed install verifies bundle hash, app id, version, publisher signature, and TOFU trust before delegating to existing app import/upgrade bundle application.
- Publisher changes for an already-installed app are refused unless a future capability adds an explicit trust migration path.
- Replay identity is provided by recorded `publish.trusted` and `publish.installed` events; app removal drops per-app provenance but keeps publisher trust.

## Deviations from the plan

- The signed `.terrane` artifact wraps Terrane's existing deterministic bundle archive with a small `TRNPUBLISH1` metadata header instead of introducing canonical tar. This keeps signing over the already-used deterministic bundle bytes and avoids a new archive implementation in the host edge.
- `publish.identity-created` exists as a validated event and folded state, but export does not currently append it. Export is an edge-only file write and the existing CLI export surface does not mutate core state. The install path records the replay-relevant public trust/provenance facts.

## Shared files touched

- `Cargo.toml`
- `Cargo.lock`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/mcp.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-api/src/lib.rs`
- `rust/crates/terrane-api/tests/contract.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Tests proving the properties

- `terrane-cap-publish/tests/capability.rs`
  - `publish_install_decides_signed_bundle_effect`
  - `publish_events_fold_trust_provenance_and_removal`
  - `publish_validation_rejects_bad_public_material`
- `rust/crates/terrane-core/tests/cap/publish.rs`
  - `publish_install_records_tofu_provenance_and_replays`
  - `publish_fold_drops_app_provenance_but_keeps_trust`
- `rust/crates/terrane-host/tests/cap/publish.rs`
  - `publish_export_install_round_trip_records_provenance_and_replays`
  - `publish_install_rejects_tampered_archive_without_log_events`
  - `publish_install_stops_on_publisher_key_change_for_existing_app`
- Contract/authz coverage:
  - `mcp_tool_surface_is_the_documented_set_with_valid_schemas`
  - `host_contract_lists_the_v1_subset`
  - `public_command_inventory_covers_every_registered_command`
- Existing compatibility coverage:
  - `deep_links::terrane_open_registered_file_imports_blob_and_delivers_reference`
  - `interop::bundle_validation_rejects_missing_common_api`
  - `net::net_request_posts_redacts_and_replays_on_loopback`

## Validation run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
