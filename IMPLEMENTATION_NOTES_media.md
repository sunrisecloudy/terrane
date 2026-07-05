# Media Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-media/` with `lib.rs`, `ops.rs`, `doc.rs`, an app example, and integration tests.
- Added host media edge support in `rust/crates/terrane-host/src/media_edge.rs`.
- Wired `Effect::MediaTransform` in `rust/crates/terrane-cap-interface/src/abi.rs` and `EdgeRunner`.
- Registered `MediaCapability` and `MediaState` in `rust/crates/terrane-core/src/lib.rs`.
- Added CLI verbs: `terrane media info <app> <name>` and `terrane media transform <app> <source> <ops-json> <dest>`.
- Regenerated the generated `ctx.resource` section in `docs/APP_API.md`.
- Added engine and host e2e tests under `rust/crates/terrane-core/tests/cap/media.rs` and `rust/crates/terrane-host/tests/cap/media.rs`.

## Key design choices

- Media bytes remain in the blob CAS. `media.transformed` records only source hash, op JSON, destination name, destination hash, size, and MIME.
- `media.transform` returns `Decision::Effect(Effect::MediaTransform { ... })`; replay folds recorded `media.transformed` and `blob.stored` events without decoding or re-encoding.
- `media.info` is a live resource read through `LiveHost::sample("media.info", ...)` and records nothing.
- Image transforms use the `image` crate. The edge checks dimensions before full decode to enforce the 64 MP pixel budget.
- Audio metadata/transcode uses `symphonia` and WAV output through `hound`, matching the v1 WAV-only plan.
- Video info uses optional `ffprobe` when available and returns `{"kind":"video","probe":"unavailable"}` when unavailable.

## Deviations from plan

- No functional deviation. The CLI verbs are additive host convenience wrappers for testing and operator use; the core capability surface remains `media.transform` plus `ctx.resource.media.info`.

## Shared files touched

- `Cargo.toml`, `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Proofs

- Crate tests:
  - `transform_decides_media_effect_from_blob_metadata`
  - `validation_errors_are_typed`
  - `transformed_event_folds_and_app_removed_clears`
- Engine tests:
  - `media_transform_records_refs_and_replays_identically`
  - `media_transform_rejects_bad_inputs_before_effect`
- Host e2e tests:
  - `media_cli_probes_and_transforms_tiny_png_through_blob_cas`
  - `media_transform_rejects_png_dimensions_over_pixel_budget`
- Inventory/docs tests updated:
  - `default_registry_exposes_registered_grant_resource_namespaces`
  - `public_command_inventory_covers_every_registered_command`
  - `grantable_command_inventory_requires_explicit_extractors_or_refusal`
  - `app_api_doc_resource_section_is_generated`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
