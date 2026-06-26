# Commit Review: db.read grants, db.query pin, CRDT rebuild primitives

Reviewed commits:

- `372e4caa fix(review 050/052): persist db.read grants across reopen; pin query.from in host`
- `c9e098d6 forge-crdt: add delete_record + incremental update export/import + from_updates rebuild (DL-4/DL-6)`

## Findings

- No new findings. `372e4caa` closes the prior fail-open grant persistence issue by loading/persisting the trusted `db.read` grant table from workspace metadata, and closes the query widening path by normalizing `query.from` to the already capability-checked collection before any bridge sees the plan. `c9e098d6` adds the CRDT delete/update-chunk/rebuild primitives with coverage for snapshot/update roundtrips, idempotent imports, out-of-order/duplicate chunk rebuilds, and garbage-input errors.

## Notes

- The first commit also addresses the follow-up from review 054: the in-memory bridge can no longer widen a two-argument `ctx.db.query("tasks", {"from":"users"})` call because `HostContext` overwrites `from` before dispatch.
- The remaining CRDT write-path orchestration work is tracked separately by `task-between-claude-and-codex/T024-crdt-write-rebuild-fixtures.md`: storage still needs one typed DL-4 function that writes CRDT chunk + oplog + projection inside a single SQLite transaction.

## Verification

- `git show --check 372e4caa`
- `git show --check c9e098d6`
- Sidecar targeted checks: `cargo test --locked -p forge-core db_read_grant_scope_persists_across_reopen`
- Sidecar targeted checks: `cargo test --locked -p forge-runtime db_query_outside_grant_is_denied`
- `cargo test --locked -p forge-crdt`
