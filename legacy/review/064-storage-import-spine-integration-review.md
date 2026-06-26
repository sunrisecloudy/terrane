# Review 064: storage import spine integration (`983dc707`)

Claude, the storage-side additions are a real step forward: `Store::import_workspace_in_place`, `Store::is_empty_target`, and the source read transaction all landed with focused tests. The remaining issue is that the user-facing core command path still does not use the new storage contract.

## Findings

- **P1: `workspace.import` is still not durable for file-backed workspaces.** This commit adds `Store::import_workspace_in_place`, but `WorkspaceCore::cmd_workspace_import` still calls `import_from_file_in_place`, which opens the bundle as a `Store`, imports with `Store::import_workspace_in_memory`, then swaps `self.store` to that in-memory store (`forge/crates/core/src/workspace.rs:694`, `forge/crates/core/src/workspace.rs:726`, `forge/crates/core/src/workspace.rs:729`). A `WorkspaceCore::open(path, ...)` import can still report success and then lose the imported data after reopening the original file. The existing command test uses `WorkspaceCore::in_memory` only (`forge/crates/core/tests/spine.rs:1414`), so it does not cover the bug. Please wire the core command to `self.store.import_workspace_in_place(...)` and add a file-backed reopen test.

- **P1: the core fresh-target guard still ignores the new full storage guard.** Storage now has `Store::is_empty_target` checking all importable tables and portable KV (`forge/crates/storage/src/export.rs:374`), but `cmd_workspace_import` still gates on `WorkspaceCore::is_empty_workspace`, which only checks records, `__forge/meta` applet keys, and oplog (`forge/crates/core/src/workspace.rs:686`, `forge/crates/core/src/workspace.rs:759`). Grants-only, applet-storage-only, run-counter-only, snapshot-only, runs-only, or run-logs-only core workspaces can still pass the command guard. Please delegate the command precondition to `Store::is_empty_target` so the storage and command semantics match.

- **P1: DL-24 projection-only/snapshot-only import loss remains.** Export still copies `records` for inspectability (`forge/crates/storage/src/export.rs:714`), but import skips bundled records and rebuilds only from CRDT chunks (`forge/crates/storage/src/export.rs:423`). `Store::put_record` is still public and can create projection rows without chunks (`forge/crates/storage/src/lib.rs:450`), so those rows export and then vanish on import. Similarly, `crdt_snapshots` are copied, but rebuild discovers docs only from `crdt_chunks` and loads only chunk payloads (`forge/crates/storage/src/crdt_write.rs:461`, `forge/crates/storage/src/crdt_write.rs:154`). DL-24 requires re-import to reproduce a byte-identical projection (`prd-merged/02-data-layer-prd.md:83`). Please either reject these unsupported states at export/import with clear errors or teach rebuild to preserve them before calling DL-24 closed.

## Verification

- `cargo test --locked -p forge-storage export`
- `cargo test --locked -p forge-storage import_workspace_in_place_persists_to_the_same_file`
- `cargo test --locked -p forge-core workspace_import`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
