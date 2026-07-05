# stream Capability Implementation Notes

## Files Changed

- Added `rust/crates/terrane-cap-stream/`
  - `Cargo.toml`
  - `src/lib.rs`
  - `src/doc.rs`
- Added stream engine tests:
  - `rust/crates/terrane-core/tests/cap/stream.rs`
  - included from `rust/crates/terrane-core/tests/cap/main.rs`
- Added host edge delivery helper:
  - `rust/crates/terrane-host/src/stream_edge.rs`
  - exported from `rust/crates/terrane-host/src/lib.rs`
- Added host CLI/e2e surface:
  - `rust/crates/terrane-host/src/cli.rs`
  - `rust/crates/terrane-host/tests/cap/stream.rs`
  - included from `rust/crates/terrane-host/tests/cap/main.rs`
- Updated public/auth/doc inventories:
  - `rust/crates/terrane-core/tests/cap/interface.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`
  - `docs/APP_API.md`
- Shared workspace/core wiring:
  - `Cargo.toml`
  - `Cargo.lock`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-host/Cargo.toml`

## Design Choices

- `stream` is a standalone capability crate with namespace `stream`, registered in `default_registry`.
- `stream.open` records desired state only. It validates app/name/verb/request, redacts sensitive headers using the same sensitive-header rules as net-v2, and records `stream.opened`.
- `stream.message`, `stream.reopened`, and `stream.close-host` are trusted-host-only through the core admission/public-authz gates. Capabilities still never set `actor`.
- Folded state is compact: `app -> name -> StreamMeta` plus the last message per stream. Replay folds events and never opens sockets or reruns JS.
- Sequence monotonicity is enforced before commit and during fold. A regression is a typed `InvalidInput` error.
- Large messages are handled at the host edge: `stream_edge::deliver_bytes` stores bytes in the blob CAS, commits `blob.link`, and records `stream.message` with `data_kind: "blob"` and `__stream__/<app>/<name>/<seq>`.
- The CLI exposes edge/testing verbs:
  - `stream open`
  - `stream close`
  - `stream ingest-text`
  - `stream reopened`
  - `stream list`

## Deviation From Plan

- The reusable host-edge delivery helper and CLI ingest path are implemented and tested, but a long-running web/mac SSE/WebSocket reconciler loop is not wired yet. This keeps the deterministic capability and recorded-effect boundary complete without inventing a new background supervisor in this slice. The folded desired-state and `stream_edge` helper are the integration points for that follow-up.

## Tests Proving Properties

- Core:
  - `stream::stream_open_redacts_request_and_replays`
  - `stream::stream_messages_are_monotonic_and_replay_identically`
  - `stream::stream_reopened_and_closed_fold_without_restreaming`
  - `stream::stream_validation_and_trusted_ingest_errors_are_typed`
  - `stream::stream_app_removed_drops_state`
  - `stream::stream_open_limit_is_enforced`
- Host/e2e:
  - `stream::stream_cli_opens_ingests_delivers_reopens_closes_and_replays`
  - `stream::stream_large_ingest_offloads_to_blob_metadata`
- Inventory/public surface:
  - `interface::default_registry_manifest_is_valid`
  - `interface::default_registry_exposes_registered_grant_resource_namespaces`
  - `interface::all_capability_docs_are_explicit_and_operational`
  - `public_command_inventory_covers_every_registered_command`
  - `grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `host::app_api_doc_resource_section_is_generated`

## Validation Run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
