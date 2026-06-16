# Review 062: core workspace export/import (`08e282c8`)

Claude, nice progress on wiring DL-24 through `forge-core`. The command path has good happy-path/RBAC coverage, but I found a few blockers before this should be treated as a real workspace import/export surface.

## Findings

- **P1: `workspace.import` into a file-backed workspace is not persisted.** `WorkspaceCore::open(path, ...)` builds a file-backed `Store`, but `cmd_workspace_import` calls `import_from_file_in_place`, which imports into `Store::import_workspace_in_memory` and then swaps `self.store` to that in-memory store (`forge/crates/core/src/workspace.rs:97`, `forge/crates/core/src/workspace.rs:726`, `forge/crates/core/src/workspace.rs:729`). A file-backed import can report success and work until process exit, then reopen the original empty target file. DL-24 says import copies/migrates into the target workspace file (`forge/spec/workspace-export-format.md:7`). Please add a file-backed import test that imports via `workspace.import`, drops/reopens the same path, and still sees the imported applet/record/grants.

- **P1: the fresh-workspace guard misses portable state, so import can overwrite a non-empty target.** `is_empty_workspace` only checks projected records, `__forge/meta` applet entries, and oplog rows (`forge/crates/core/src/workspace.rs:756`). It ignores existing KV (`applet/*` storage, `__forge/meta/db_read_grants`, `run_counter`), CRDT chunks/snapshots, runs, and run logs. That allows a grants-only or KV-only workspace to pass the “fresh” check and have its state silently replaced in memory. Please either delegate the precondition to a storage-level empty-target import or check all syncable/importable tables/namespaces.

- **P1: the core command exposes the storage export atomicity bug from review 061.** `workspace.export` now advertises the bundle as deterministic/portable but still delegates to `Store::export_workspace` (`forge/crates/core/src/workspace.rs:640`), whose `write_bundle` copies tables one by one without a source read transaction/snapshot or destination transaction (`forge/crates/storage/src/export.rs:203`). Concurrent writes can produce split-brain bundles, and a failure can leave a partial target file. This needs fixing in storage before the core command is safe, plus a concurrent-write/export regression.

- **P2: the command contract/spec drifted from the implementation.** `forge/spec/commands.md:9` still lists `workspace.export` payload fields as `format_version?, include_run_logs?` and a returned artifact descriptor/bytes handle, while the implementation requires a raw filesystem `path` and writes to it (`forge/crates/core/src/workspace.rs:575`). Either update the spec to make shell-brokered paths explicit, or change the command to return a brokered artifact handle so authorized actors do not get an arbitrary file-create primitive by accident.

- **P2: the export descriptor overclaims DL-24 completeness.** The response marks broad sections as included, but DL-24 still requires applet sources, schemas, index definitions, RBAC config, permissions, and provenance (`prd-merged/02-data-layer-prd.md:83`); `forge/spec/workspace-export-format.md:24` says missing GA-required sections should be reported as `missing_required_for_ga`. Please make the descriptor honest until those tables exist.

## Verification

- `cargo test --locked -p forge-core export`
- `cargo test --locked -p forge-core workspace_import`
- `cargo test --locked -p forge-core db_read_grant_scope_persists_across_reopen`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
