# Implementation Notes: connection

## Files changed

- Added `rust/crates/terrane-cap-connection/` with metadata state, commands, fold, describe, docs, validation helpers, and integration tests.
- Wired the new capability into `Cargo.toml`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`, and `rust/crates/terrane-host/Cargo.toml`.
- Added `rust/crates/terrane-host/src/secret_store.rs` and exported it from `terrane-host`.
- Wired `$secret` resolution into `rust/crates/terrane-host/src/edge.rs` for `Effect::HttpRequest`.
- Added trusted operator CLI verbs in `rust/crates/terrane-host/src/cli.rs`: `connection set`, `connection ls`, `connection stat`, `connection rm`, and a placeholder `connection authorize` error.
- Extended `terrane-cap-auth` narrowly so `auth.grant user app connection:<name>` records a per-name `connection:<name>` grant resource.
- Updated net v2 request redaction so `$secret` marker objects remain verbatim in recorded redacted request JSON while raw string secrets are still redacted.
- Updated public authz and capability/resource docs inventories, including `docs/APP_API.md`.
- Added engine and host e2e tests for metadata replay, marker recording, per-name grant enforcement, encrypted fallback store, and loopback net resolution.

## Key design choices

- Replay state stores only public metadata: name, kind, public config, authorized flag, scopes, and expiry.
- Secret bytes enter only through host CLI stdin/prompt and are written to the host store before `connection.define` dispatches.
- The host store tries OS keychain first via `keyring`; if unavailable or forced with `TERRANE_SECRET_STORE=file`, it uses `$TERRANE_HOME/secrets.enc` sealed with `terrane-cap-crypto` primitives and a local `0600` host key file.
- `net.request` prepares the original marker-bearing JSON for `request_key` and recorded event data, then prepares a resolved in-memory copy for the actual HTTP effect.
- Per-name grants use `connection:<name>` resource ids. Broad `connection` grants are not required for secret resolution.

## Deviations / follow-up

- OAuth browser/loopback authorization and refresh-on-demand are scaffolded in the capability metadata model, but the actual OAuth code exchange/refresh flow is not implemented in this slice. `terrane connection authorize <name>` currently returns an explicit not-wired error.
- Secret deletion is performed by the trusted CLI `connection rm` path. There is no generic fold hook that can delete host side artifacts for every possible host process yet.

## Shared files touched

- Root `Cargo.toml` and `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-cap-auth/Cargo.toml`
- `rust/crates/terrane-cap-auth/src/lib.rs`
- `rust/crates/terrane-cap-net/src/request.rs`

## Proof tests

- Metadata and replay identity: `connection::connection_metadata_folds_and_replays_without_secret_material`
- Marker-verbatim recording: `connection::net_request_records_secret_marker_verbatim_for_stable_request_identity`
- Input validation: `validates_names_public_config_and_secret_refs`
- File fallback + net edge resolution + no log leak: `connection::connection_file_fallback_resolves_secret_for_net_without_log_leak`
- Missing per-name grant blocks resolution: `connection::missing_connection_grant_blocks_resolution`
- Public authz classification: `public_command_inventory_covers_every_registered_command`, `grantable_command_inventory_requires_explicit_extractors_or_refusal`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
