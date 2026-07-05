# app-update Implementation Notes

## Files changed

- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-cap-app/src/lib.rs`
- `rust/crates/terrane-cap-app/src/doc.rs`
- `rust/crates/terrane-cap-app/tests/capability.rs`
- `rust/crates/terrane-cap-js-runtime/src/bundle.rs`
- `rust/crates/terrane-core/tests/cap/app.rs`
- `rust/crates/terrane-core/tests/cap/host.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/mcp.rs`
- `rust/crates/terrane-host/src/preview.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/app.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `rust/crates/terrane-api/src/lib.rs`
- `rust/crates/terrane-api/tests/contract.rs`

## Key design choices

- Extended the existing `app` namespace with `app.upgrade` and `app.upgraded`; no new capability crate or central command/event enum was added.
- Added `Effect::UpgradeAppBundle { id, source }`; the capability decides only app existence and input shape, while the host edge performs filesystem, draft, blob CAS, and migration work.
- Added manifest `version` parsing with default `0.0.0`; semver validation is owned by `terrane-cap-app`.
- Kept merged schema-migration behavior for data versions: omitted or zero `dataVersion` is interpreted as the existing migration default version `1`.
- Upgrade returns one atomic event batch: migration runtime resource records plus `migration.applied`, outgoing/incoming `blob.stored` archive metadata, `app.upgraded`, then `kv.set`/`kv.deleted` bundle file diff events.
- Migration scripts are executed during upgrade through the same JS migration runtime path and record ordinary resource-write events before the `migration.applied` fact.
- Bundle archives are deterministic text-bundle archives stored in the blob CAS under names `__app__/<id>/<version>`.
- `--from-draft` resolves existing MCP app-builder drafts under `$TERRANE_HOME/.mcp-drafts/<draft>/bundle`.
- `--to-version` reinstalls an archived bundle from blob metadata and CAS bytes.
- `app.upgrade` is explicitly refused on the public `capability_command` path and exposed as a purpose-built trusted MCP tool `app_upgrade`.

## Deviations from the plan

- The merged migration capability defaults app data version to `1`, and host/runtime code already treated omitted `dataVersion` that way. I preserved that merged behavior instead of changing defaults to `0`.
- `app.upgraded.bundle_hash` records the SHA-256 of the canonical archive bytes. This keeps the recorded bundle hash identical to the CAS hash used for `--to-version`.
- Version history in `AppRecord` tracks folded upgrade targets. Historical bundle bytes for both outgoing and incoming versions are available through blob metadata even when the older version was not itself an `app.upgraded` target.

## Shared files touched

- Public ABI: `rust/crates/terrane-cap-interface/src/abi.rs`
- Host edge/CLI/MCP surfaces: `rust/crates/terrane-host/src/{edge.rs,cli.rs,mcp.rs,lib.rs,public_authz.rs}`
- API contract: `rust/crates/terrane-api/src/lib.rs`
- Core/host test fixtures that directly construct `AppRecord`

## Proof tests

- Command/effect surface and validation: `terrane-cap-app tests::app_upgrade_decides_effect_and_folds_version_history`
- Missing app and semver validation: `terrane-cap-app tests::app_upgrade_rejects_missing_apps_and_bad_versions`
- Capability docs include upgrade surface: `terrane-cap-app tests::app_doc_covers_manifest_and_removal_cleanup_boundary`
- Replay identity for an upgrade plus migration/KV batch: `terrane-core cap::app::upgrade_effect_batch_replays_identically`
- Real binary bundle upgrade, migration execution, file deletion, CAS archive read, same-version rejection, and replay check: `terrane-host cap::app_upgrade_e2e_replaces_bundle_runs_migration_and_archives_versions`
- MCP tool contract includes `app_upgrade`: `terrane-api contract::mcp_tool_surface_is_the_documented_set_with_valid_schemas`
- Public command authorization explicitly covers and refuses `app.upgrade`: `terrane-host public_authz::public_command_inventory_covers_every_registered_command`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
