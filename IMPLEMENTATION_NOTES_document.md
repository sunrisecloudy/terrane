# document Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-document/` with `lib.rs`, `types.rs`,
  `events.rs`, `resources.rs`, `doc.rs`, schemas, examples, and integration
  tests.
- Added engine tests in `rust/crates/terrane-core/tests/cap/document.rs` and
  wired them through `rust/crates/terrane-core/tests/cap/main.rs`.
- Added host e2e tests in `rust/crates/terrane-host/tests/cap/document.rs` and
  wired them through `rust/crates/terrane-host/tests/cap/main.rs`.
- Updated `docs/APP_API.md` generated resource table for
  `ctx.resource.document`.
- Retired the old planned document doc and moved its schemas/examples into the
  live capability crate.

## Design choices

- Implemented the frozen namespace and names exactly:
  `document.create`, `document.patch`, `document.append`, `document.delete`,
  with events `document.created`, `document.patched`, `document.deleted`.
- `DocumentState` is pure folded state under `terrane_core::State.document`.
  There is no physical projection or parallel store in v1.
- `document.patch` replaces `title` and `body` fields wholesale and applies
  RFC 7386 merge-patch semantics to `metadata`.
- `document.append` emits `document.patched` with an `append` field, preserving
  the planned three-event surface.
- App removal is handled through broadcast fold of `app.removed`.
- `ctx.resource.document` is grant-gated with namespace-v1 `read`/`write`.
  Public command authorization gates the document write commands on that grant.

## Shared files touched

- Root `Cargo.toml` and `Cargo.lock`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/src/planned_docs/mod.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- Public-surface/inventory tests under `rust/crates/terrane-core/tests/cap/`,
  `rust/crates/terrane-host/tests/`, and `host/cli/tests/`

## Deviations

- None from the recommended plan. The v1 implementation is folded-log only and
  does not add a reserved KV or SQLite projection.

## Proof tests

- `document_capability_decides_folds_and_reads_public_surface`
- `document_capability_enforces_per_app_quota_without_global_store`
- `document_create_patch_append_delete_replays_identically`
- `document_create_replaces_and_delete_missing_is_noop`
- `document_validation_errors_are_typed`
- `document_quota_is_enforced`
- `removing_the_app_drops_its_documents`
- `document_e2e_runs_js_backend_and_cli_reads_folded_state`
- `public_command_inventory_covers_every_registered_command`
- `grantable_command_inventory_requires_explicit_extractors_or_refusal`
- `app_api_doc_resource_section_is_generated`

## Gate

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
