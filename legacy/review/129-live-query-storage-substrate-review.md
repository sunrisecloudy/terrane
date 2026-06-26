# Review 129: live-query storage substrate

Reviewed commit `3c500cc3` (`forge-storage: db.watch registry + deterministic dirty-set + notification computation (DL-16)`).

## Findings

- **P1: The mixed-collection T047 transaction fixtures pass without exercising an actual atomic transaction.** The new fixture runner explicitly splits a multi-collection `transact` into one CRDT write per collection, then folds those independent writes into one registry commit/version (`forge/crates/storage/tests/live_query_fixtures.rs:453-481`). The real CRDT write path still rejects a single group that spans collections (`forge/crates/storage/src/crdt_write/mutation.rs:233-250`), while the PRD/query specs say `transact([...])` commits all included mutations as one local SQLite transaction and rolls the whole group back on failure (`prd-merged/02-data-layer-prd.md:68-70`, `forge/spec/query-dsl.md:89-97`), and live queries require one dirty set per committed `transact` (`forge/spec/live-queries.md:55-61`). As written, the T047 cases for tasks+notes can pass even though a later collection write could fail after an earlier collection already committed, leaving no single dirty set or rollback boundary. Please either implement a real multi-doc/multi-collection atomic write boundary before claiming these fixtures, or mark/split the cross-collection fixtures as unsupported instead of passing them through the split-write harness.

- **P2: Aggregate/group watches are silently accepted but never notify.** The query spec includes aggregates and `groupBy` in the M0a query AST and lists `watch(query, cb)` as the same query AST as `all()` (`forge/spec/query-dsl.md:32-48`). `WatchRegistry::register_from_value` accepts any parsed `Query`, but `run_watch_ids` turns `QueryResult::Aggregate` and `QueryResult::Groups` into an empty id list (`forge/crates/storage/src/watch.rs:527-534`); `commit` only emits when a dirty id appears in the before/after id lists (`forge/crates/storage/src/watch.rs:493-505`). That means a `db.watch` on `{from:"tasks", aggregate:{count:true}}` would stay active and receive no notifications as the count changes. Please reject aggregate/group queries at watch registration with a `QueryError` until the notification payload has a defined non-row-result shape, and add coverage for the rejection.

## Verification

- `cargo test -p forge-storage --test live_query_fixtures`
- `cargo test -p forge-storage --lib watch`
- `cargo clippy -p forge-storage -- -D warnings`
