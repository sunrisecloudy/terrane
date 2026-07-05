# Blob Capability Implementation Notes

## Files changed

- `Cargo.toml`, `Cargo.lock`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-core/tests/cap/blob.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-cap-blob/Cargo.toml`
- `rust/crates/terrane-cap-blob/src/lib.rs`
- `rust/crates/terrane-cap-blob/src/doc.rs`
- `rust/crates/terrane-cap-blob/src/util.rs`
- `rust/crates/terrane-cap-blob/examples/blob_resource_methods.js`
- `rust/crates/terrane-cap-blob/tests/capability.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/blob_store.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/ffi.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/include/terrane_host.h`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/cap/blob.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `host/web/Cargo.toml`
- `host/web/src/js/terrane_shim.js`
- `host/web/src/routes.rs`
- `host/macos/Sources/AppDelegate.swift`
- `host/macos/Sources/AppSchemeHandler.swift`
- `host/macos/Sources/TerraneBridge.swift`
- `host/macos/Tests/TopBarProtocolTests.swift`
- `docs/APP_API.md`

## Key design choices

- Added `terrane-cap-blob` as a deterministic capability. The pure capability records only metadata events:
  - `blob.stored { app, name, hash, size, mime }`
  - `blob.removed { app, name, hash }`
- `blob.put` validates app/name/mime/size, decodes base64, computes SHA-256 in `decide`, and returns `Effect::BlobStore`. The event is produced only after the edge writes or confirms the content-addressed sidecar row.
- The SQLite sidecar lives at `storage_home/blobs.sqlite3` through the same host-home plumbing used by existing sidecar storage. The table is keyed by lowercase SHA-256 hex and stores raw bytes separately from the event log.
- Reads verify the row hash and size before returning bytes. Missing rows, invalid hashes, corrupt rows, and oversized inputs return typed errors instead of panicking.
- Replay only folds blob metadata and refcounts. It never rewrites bytes. Missing bytes are detected at read/verify time.
- `blob.link` exists for sync/import metadata repair when bytes are copied separately; it records metadata by hash without needing local bytes at decide time.
- CLI, web, and macOS surfaces all read through the verified CAS path:
  - CLI: `blob put|get|ls|stat|rm|verify|gc`
  - Web: `window.terrane.blobUrl(name)` and `GET /apps/{id}/blob/{name}` gated by the app's `blob` grant
  - macOS: `terrane_blob_read` plus `terrane-app://.../blob/<name>`
- Public command authz classifies `blob.put`, `blob.rm`, and `blob.link` as grant-gated on app arg 0 and namespace `blob`.

## Deviations from the plan

- Sync copies blob metadata through `blob.link` and then copies all live source hashes from the source home into the local CAS. This preserves the plan's metadata-before-bytes ordering and lets a second sync heal missing sidecar bytes without replaying byte writes.
- `blob.stat` is implemented as a resource read/CLI helper rather than a separate committed command, because it is derived from folded metadata and verified CAS state.

## Shared files touched

- Workspace wiring: root `Cargo.toml`, `Cargo.lock`
- Capability ABI: `rust/crates/terrane-cap-interface/src/abi.rs`
- Core registry/state/doc generation: `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`, `docs/APP_API.md`
- Host edge/CLI/authz/FFI: `rust/crates/terrane-host/*`
- Web and macOS host adapters: `host/web/*`, `host/macos/*`

## Tests proving the properties

- Pure capability:
  - `rust/crates/terrane-cap-blob/tests/capability.rs::put_effect_contains_bytes_but_event_does_not`
  - `rust/crates/terrane-core/tests/cap/blob.rs::blob_put_records_metadata_only_and_replays_identically`
  - `rust/crates/terrane-core/tests/cap/blob.rs::blob_rm_and_app_removed_update_refcounts`
  - `rust/crates/terrane-core/tests/cap/blob.rs::blob_link_records_metadata_without_cas_presence_check`
  - `rust/crates/terrane-core/tests/cap/blob.rs::blob_validation_errors_are_typed`
- Host/e2e:
  - `rust/crates/terrane-host/tests/cap/blob.rs::blob_cli_round_trip_uses_verified_sqlite_cas`
  - `rust/crates/terrane-host/tests/cap/blob.rs::blob_verify_reports_corrupt_bytes_without_panicking`
  - `rust/crates/terrane-host/tests/cap/blob.rs::blob_gc_dry_run_reports_unreferenced_rows`
  - `rust/crates/terrane-host/tests/cap/blob.rs::sync_from_home_copies_blob_metadata_and_sidecar_bytes`
- Registry/authz/doc coverage:
  - `rust/crates/terrane-core/tests/cap/interface.rs::default_registry_exposes_registered_grant_resource_namespaces`
  - `rust/crates/terrane-host/tests/public_authz.rs::grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `rust/crates/terrane-host/tests/public_authz.rs::public_command_inventory_covers_every_registered_command`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
