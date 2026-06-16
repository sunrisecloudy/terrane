# Commit Review: 7dab0bb7 core grant scope + replay fallback

Reviewed commit: `7dab0bb7 forge-core: close review 038 (collection-scoped db.read grant, write-once code_hash fallback) +2 tests, 34 green`

## Findings

1. **P1 - `query.execute` still trusts caller-supplied grants, so the scoped `db.read` check is bypassable.** The new gate reads the capability scope from `cmd.payload.grants.db.read` (`forge/crates/core/src/workspace.rs:826`, `forge/crates/core/src/workspace.rs:866`) and treats a missing scope as role-derived read-all (`forge/crates/core/src/workspace.rs:850`). That means any role that already passes the role gate can omit `grants` or submit `{"db":{"read":["*"]}}` in the request body and read a collection that should be outside its actual grant. The new regression only proves denial when the caller voluntarily includes the restrictive fixture grant (`forge/crates/core/tests/spine.rs:467`); it does not prove the boundary is trusted. Please derive the `db.read` scope from a trusted permission/session/manifest context rather than from the request payload, and add a regression for the same ungranted collection where the request omits or self-expands `grants`.

## Notes

- The write-once `program/<code_hash>` fallback change and legacy replay regression cover review 038's replay fallback concern; I did not find a new issue there.

## Verification

- `git show --check 7dab0bb7`
- `(cd forge && cargo test --locked -p forge-core query_execute_enforces_collection_scoped_db_read_grant)`
- `(cd forge && cargo test --locked -p forge-core legacy_run_on_codehash_fallback_replays_after_same_code_tighter_reinstall)`
- `(cd forge && cargo test --locked -p forge-core)`
- `(cd forge && cargo clippy --locked -p forge-core --all-targets -- -D warnings)`
