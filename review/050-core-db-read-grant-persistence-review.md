# Commit Review: a956c776 core trusted db.read grants

Reviewed commit: `a956c776 forge-core: close review 048 (db.read scope from trusted grant table, not request payload; reject payload self-escalation) +1 test, 35 green`

## Findings

1. **P1 - Scoped `db.read` grants disappear on reopen and become read-all.** The new trusted grant table is only an in-memory `BTreeMap` (`forge/crates/core/src/workspace.rs:64`), and both `WorkspaceCore::open` and `WorkspaceCore::in_memory` initialize it empty (`forge/crates/core/src/workspace.rs:77`, `forge/crates/core/src/workspace.rs:88`). But an absent entry is treated as role-derived read-all (`forge/crates/core/src/workspace.rs:868`, `forge/crates/core/src/workspace.rs:894`). That means any file-backed workspace with a scoped grant configured through `grant_db_read("dev", ["tasks"])` loses that trusted scope after `WorkspaceCore::open(...)`; the same actor can then query `secrets` because "missing grant row" means unrestricted access. Please persist/load the trusted grant state or change absence to deny/empty scope once the grant system is active, and add a file-backed reopen regression proving a scoped actor remains scoped after reopening.

## Notes

- The request-payload self-escalation regression closes the direct bypass from review 048 when a trusted grant entry is present.

## Verification

- `git show --check a956c776`
- `(cd forge && cargo test --locked -p forge-core query_execute_db_read_scope_is_not_forgeable_from_payload)`
- `(cd forge && cargo test --locked -p forge-core query_execute_enforces_collection_scoped_db_read_grant)`
- `(cd forge && cargo test --locked -p forge-core)`
- `(cd forge && cargo clippy --locked -p forge-core --all-targets -- -D warnings)`
