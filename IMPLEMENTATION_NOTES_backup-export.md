# Implementation Notes: backup-export

## Files changed

- `rust/crates/terrane-host/src/backup.rs`: host-owned archive create/info/restore/export/import implementation.
- `rust/crates/terrane-host/src/cli.rs`: added `terrane backup create|info|restore`, `terrane export`, and `terrane import`.
- `rust/crates/terrane-host/src/lib.rs`: exported the backup module.
- `rust/crates/terrane-host/tests/cap/backup.rs`: e2e coverage for backup/export/import.
- `rust/crates/terrane-host/tests/cap/main.rs`: included the backup e2e module.
- `rust/crates/terrane-host/src/public_authz.rs` and `rust/crates/terrane-host/tests/public_authz.rs`: classified `replica.rotate` as trusted-host-only and updated command inventory.
- `rust/crates/terrane-core/src/lib.rs`: added `Core::append_recorded`, `Core::app_of_record`, and public `write_log` for host archive/import flows.
- `rust/crates/terrane-cap-interface/src/capability.rs`: added default `Capability::app_of`.
- `rust/crates/terrane-cap-app/src/lib.rs`, `rust/crates/terrane-cap-kv/src/{lib.rs,events.rs}`, `rust/crates/terrane-cap-crdt/src/{lib.rs,events.rs}`, `rust/crates/terrane-cap-blob/src/lib.rs`: implemented app ownership for app-scoped event slicing.
- `rust/crates/terrane-cap-replica/src/{lib.rs,doc.rs,tests.rs}` and `rust/crates/terrane-cap-replica/tests/capability.rs`: added `replica.rotate` and documented/tested latest recorded peer identity.
- `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-host/Cargo.toml`: added `tar`, `zstd`, and direct host `serde` dependency wiring.

## Design choices

- Backup/export are host-level operations only. No capability crate, namespace, or user app surface was added.
- Archives are zstd-compressed tar files with `manifest.json` written first, followed by hash-bound files.
- Restore verifies every manifest file hash, log record count, and in-memory replay fold before copying into the target home.
- Restore refuses non-empty targets. Plain restore preserves replica identity; `--clone` dispatches trusted `replica.rotate` after restore.
- Import appends already-recorded events through `Core::append_recorded` so payloads and actors are preserved instead of re-deciding private commands.
- Per-app export uses `Capability::app_of` over raw events. V1 ownership is implemented for `app`, `kv`, `crdt`, and `blob`, which covers the current app lifecycle/data/blob export path.
- Blob CAS bytes are included conditionally: full backup includes the home CAS when present; app export copies only live hashes referenced by the app's folded blob state.
- `replica.rotate` is explicitly refused on public/MCP capability-command surfaces because it is clone/restore identity repair, not app-callable behavior.

## Deviations / notes

- The plan called out SQLite backup API / `VACUUM INTO`; full backup uses `VACUUM INTO` for `blobs.sqlite3`.
- Archive construction uses temporary directories for verified restore/import and sliced export logs. The CLI streams tar to zstd for the final archive, but file metadata is precomputed for manifest hashes.
- Existing replica inline tests were updated in place because that crate already had inline tests; no new inline test module was introduced.

## Shared files touched

- Root workspace dependency metadata: `Cargo.toml`, `Cargo.lock`.
- Core public API: `rust/crates/terrane-core/src/lib.rs`.
- Capability trait: `rust/crates/terrane-cap-interface/src/capability.rs`.
- Public authz inventory: `rust/crates/terrane-host/src/public_authz.rs`, `rust/crates/terrane-host/tests/public_authz.rs`.
- CLI help/parser: `rust/crates/terrane-host/src/cli.rs`.

## Proof tests

- `backup::backup_create_restore_preserves_replay_state`: create -> restore, replay identity, and restored state equality.
- `backup::backup_restore_refuses_nonempty_target`: restore safety rule.
- `backup::backup_restore_tamper_is_rejected`: manifest/hash/decode tamper rejection.
- `backup::backup_restore_clone_rotates_peer`: `--clone` changes replica peer.
- `backup::export_import_round_trips_one_app_and_refuses_existing_id`: app export/import round-trip and duplicate-id refusal.
- `public_command_inventory_covers_every_registered_command`: `replica.rotate` is classified and inventory stays explicit.
- `replica_capability_initializes_and_queries_peer` / `replica_doc_covers_stable_identity_and_effect_boundary`: replica command/doc surface includes rotate.

## Validation run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
