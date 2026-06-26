# Commit Review: 6c3d0cbd

Reviewed commit: `6c3d0cbd forge-core: close review 031 (RBAC gate, unique run_id, version-pinned replay)`

## Findings

1. **P1 - `query.execute` still bypasses the required `db.read` capability.** The new command gate authorizes by role only (`forge/crates/core/src/workspace.rs:631`), then `cmd_query_execute` reads any named collection directly via `self.store.list_records(collection)` (`forge/crates/core/src/workspace.rs:451`). `forge/spec/commands.md:21` requires "Role plus db.read capability", and `prd-merged/01-core-runtime-prd.md:36` requires every command to pass RBAC/capability validation before touching state. Add a command-level query policy/capability check before listing records, and add a negative test for a read role without `db.read`.

2. **P1 - Replay artifacts are keyed only by JS `code_hash`, so same-code manifest revisions can overwrite the context old runs need.** `store_program` writes `InstalledApplet` at `program/{code_hash}` (`forge/crates/core/src/workspace.rs:495`, `forge/crates/core/src/workspace.rs:549`), and replay loads by `original.code_hash` only (`forge/crates/core/src/workspace.rs:397`). If the same compiled JS is reinstalled with tighter `limits` or different legacy capabilities, the pinned artifact for older runs is overwritten; replay still uses the loaded manifest's engine limits (`forge/crates/runtime/src/runner.rs:174`, `forge/crates/runtime/src/runner.rs:209`). Pin by run/version or by `(code_hash, manifest_hash)`, and cover reinstalling identical JS with changed limits/caps before replaying an old run.

3. **P2 - The persisted run counter is not atomic across multiple workspace opens.** `next_run_counter` does a separate KV read and write (`forge/crates/core/src/workspace.rs:523`, `forge/crates/core/src/workspace.rs:536`) before the run is saved, while `save_run` overwrites on `run_id` conflict (`forge/crates/storage/src/lib.rs:520`). Two `WorkspaceCore::open()` instances can reserve the same invocation and replace one audit record. Move counter reservation into an atomic SQLite update/transaction tied to run persistence, or make `runs.run_id` conflicts fail loudly.

## Verification

- `git show --check 6c3d0cbd` passed.
- `cargo test --locked -p forge-core` passed.
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings` passed.
- `cargo run --locked -p forge-cli -- demo` passed.
