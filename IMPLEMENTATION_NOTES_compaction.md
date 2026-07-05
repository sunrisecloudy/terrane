# Compaction Implementation Notes

## Files changed

- `rust/crates/terrane-cap-interface/src/capability.rs`
- `rust/crates/terrane-cap-interface/src/state.rs`
- `rust/crates/terrane-cap-interface/src/lib.rs`
- `rust/crates/terrane-core/src/snapshot.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `Cargo.lock`
- Snapshot/restore support in stateful slices exercised by the default host/core path:
  - `rust/crates/terrane-cap-app/src/lib.rs`
  - `rust/crates/terrane-cap-auth/src/lib.rs`
  - `rust/crates/terrane-cap-blob/src/lib.rs`
  - `rust/crates/terrane-cap-crdt/src/lib.rs`
  - `rust/crates/terrane-cap-crdt/src/state.rs`
  - `rust/crates/terrane-cap-history/src/lib.rs`
  - `rust/crates/terrane-cap-kv/src/lib.rs`
  - `rust/crates/terrane-cap-kv/src/types.rs`
  - `rust/crates/terrane-cap-person/src/lib.rs`
  - `rust/crates/terrane-cap-query/src/lib.rs`
  - `rust/crates/terrane-cap-replica/src/lib.rs`
- Host CLI:
  - `rust/crates/terrane-host/src/cli.rs`
- Tests:
  - `rust/crates/terrane-core/tests/cap/compaction.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/cap/compaction.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`

## Key design choices

- Added optional `Capability::snapshot` / `Capability::restore` hooks. Sections are stored by namespace, so core does not centralize capability-private snapshot layouts.
- Added `snapshot.bin` with a fixed header, `format_version`, folded `seq`, archived-log SHA-256, and Borsh section framing.
- `Core::open_with` now restores `snapshot.bin` first, then folds the retained live `log.bin` tail. `read_log` still reads only the live log frames.
- `compact_log` snapshots the prefix up to `len - retain`, writes a retained tail log, verifies `restore(snapshot) + tail == full replay`, copies the old full log to `log.bin.archive`, then swaps files.
- `log.bin.archive` is retained by default. `--prune-archive` explicitly removes a previous archive before compacting again.
- CRDT snapshots use Loro `ExportMode::Snapshot` and import into fresh docs, preserving version-vector delta export after compaction.
- Blob GC safety comes from the existing home lock plus replay-equivalent folded blob refcounts before and after compaction; compaction does not touch `blobs.sqlite3`.

## Deviations

- `--verify` is accepted for the planned CLI surface, but verification is always performed before swapping files. This is stricter than an optional preflight and keeps compaction from ever silently corrupting a home.
- The swap sequence copies the old log to `log.bin.archive` before publishing the snapshot/tail pair. On open, if an archive exists and the live log count still looks like the full old log rather than the retained tail, the snapshot is ignored as an incomplete compaction.

## Shared files touched

- `Cargo.lock`
- `rust/crates/terrane-cap-interface/src/capability.rs`
- `rust/crates/terrane-cap-interface/src/state.rs`
- `rust/crates/terrane-cap-interface/src/lib.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`

## Test coverage

- Core compaction tests:
  - `compaction::compaction_reopens_to_same_state_and_retains_tail_identity`
  - `compaction::compaction_with_zero_retain_replays_from_snapshot_only`
  - `compaction::unknown_snapshot_section_is_a_storage_error`
  - `compaction::leftover_tmp_files_are_ignored_on_open`
  - `compaction::crdt_snapshot_preserves_version_vector_delta_export`
- Host e2e tests:
  - `compaction::compact_cli_snapshots_tail_and_home_remains_usable`
  - `compaction::compact_cli_rejects_bad_retain_value`

## Validation run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
