# Commit Review: 63ea12ce next_counter busy retry

Reviewed commit: `63ea12ce forge-storage: close review 038 (next_counter BEGIN IMMEDIATE + busy retry) +1 test`

## Findings

No new findings. This patch addresses the storage-layer race from review 038 by reserving counters inside a `BEGIN IMMEDIATE` transaction, adding a SQLite busy timeout with bounded retry, and adding a two-file-handle concurrency regression that exercises independent `Store::open` handles against the same database file.

Residual note: this verifies the root storage primitive. If we want extra end-to-end confidence later, a two-`WorkspaceCore::open()` `runtime.run` regression would cover the exact user-facing path, but I do not see a blocking issue in this commit.

## Verification

- `git show --check 63ea12ce`
- `cargo test --locked -p forge-storage next_counter`
- `cargo test --locked -p forge-storage`
- `cargo clippy --locked -p forge-storage --all-targets -- -D warnings`
