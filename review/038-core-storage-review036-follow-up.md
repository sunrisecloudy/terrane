# Commit Review: ac5aa7d6..21fa636e

Reviewed commits:

- `ac5aa7d6 forge-storage: atomic next_counter primitive for unique run ids (review 036) +2 tests`
- `21fa636e forge-core: close review 031/036 (db.read cap gate, per-run replay pin, atomic run counter) +2 tests`

## Findings

1. **P1 - `query.execute` still does not enforce an independent `db.read` capability.** The new capability helper grants exactly the same role set that `authorize` already allows for `query.execute`: Owner/Maintainer/Editor/Viewer/Auditor (`forge/crates/core/src/workspace.rs:755`, `forge/crates/core/src/workspace.rs:781`). That makes `require_db_read` redundant rather than "Role plus db.read capability"; the added negative test with Runner is already rejected by the earlier role gate. `forge/spec/capabilities.md:23` models `db.read` as a collection/query-scoped capability, so review 036 finding 1 remains open until the command path checks an actual grant/scope before `list_records`.

2. **P2 - The legacy replay fallback remains mutable for pre-per-run-pin runs.** New runs get `program/run/<run_id>`, but every run still overwrites the old `program/<code_hash>` artifact (`forge/crates/core/src/workspace.rs:355`, `forge/crates/core/src/workspace.rs:543`), and replay falls back to that mutable artifact for runs without per-run pins (`forge/crates/core/src/workspace.rs:429`). A run recorded before this commit can still be stranded by a later same-JS reinstall under a different manifest. Make the code-hash fallback write-once, key it by manifest/version, or migrate legacy runs to a per-run artifact before overwriting the fallback.

3. **P2 - `next_counter` is not proven safe for the two-handle race review 036 called out.** `Store::next_counter` uses the default deferred SQLite transaction (`forge/crates/storage/src/lib.rs:177`) and performs SELECT-before-upsert (`forge/crates/storage/src/lib.rs:267`). Two file-backed connections can both read the same snapshot; the loser may surface `database is locked` / `StorageError` instead of observing the first committed value. The tests are sequential reopen/corruption checks only (`forge/crates/storage/src/lib.rs:848`). Use `BEGIN IMMEDIATE`, an atomic update/returning pattern with retry, and add a two-`WorkspaceCore::open()` concurrent `runtime.run` regression.

## Verification

- `git show --check ac5aa7d6` passed.
- `git show --check 21fa636e` passed.
- `cargo test --locked -p forge-storage` passed.
- `cargo test --locked -p forge-core` passed.
- `cargo clippy --locked -p forge-storage --all-targets -- -D warnings` passed.
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings` passed.
