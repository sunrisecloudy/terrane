# Review 117: core RBAC extraction

Reviewed commit `f40648c9` (`forge-core: extract RBAC authorization into auth.rs`).

## Findings

- No blocking findings. The command-level role matrix and `query.execute` db-read scope helpers moved into `forge/crates/core/src/auth.rs`; `WorkspaceCore::handle` still calls `authorize` before dispatch, and query execution still uses the trusted grant table via `require_db_read`.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
