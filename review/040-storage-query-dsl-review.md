# Commit Review: 0e84bf9e storage query DSL

Reviewed commit:
- `0e84bf9e forge-storage: query DSL + planner + mutations over records projection (DL-15/16/17)`

## Findings

1. **P1 - Mutations update only the projection, so rebuild/sync will lose writes.** DL-4/DL-17 require the write path to create a CRDT op, append `crdt_chunks`/`oplog`, and update the projection in one SQLite transaction (`prd-merged/02-data-layer-prd.md:49`, `forge/spec/query-dsl.md:92`, `forge/spec/query-dsl.md:96`). This commit's mutation APIs write directly to `records` through `put_record`/`put_record_tx` (`forge/crates/storage/src/lib.rs:540`, `forge/crates/storage/src/lib.rs:599`, `forge/crates/storage/src/lib.rs:957`), and the new tests only assert projection rows (`forge/crates/storage/tests/query_fixtures.rs:239`, `forge/crates/storage/tests/query_fixtures.rs:276`). Please either route these mutations through the CRDT/oplog path or keep them out of the applet mutation surface, then add tests that assert the expected oplog/CRDT commit is created.

2. **P1 - Field-id queries and indexes compile against display fields.** `field_id` predicates are parsed into the same plain `field` string as display-name predicates (`forge/crates/storage/src/query.rs:312`), and every field path resolves to `$.fields.<field>` (`forge/crates/storage/src/query.rs:452`, `forge/crates/storage/src/query.rs:487`). That contradicts the stable field-id query/index contract (`forge/spec/query-dsl.md:16`, `prd-merged/02-data-layer-prd.md:50`) and can make field-id index fixtures pass against the wrong JSON path. Introduce a real field reference/schematic resolver and compile stable ids to `$.field_ids.<id>` with tests that fail if the display field path is used.

3. **P1 - Equality and `IN` still coerce booleans/numbers.** `eq`/`ne` and `in` compare raw `json_extract` values without a `json_type` guard (`forge/crates/storage/src/query.rs:501`, `forge/crates/storage/src/query.rs:548`), while boolean bind params are converted to SQLite integers (`forge/crates/storage/src/lib.rs:871`). SQLite will therefore match `false` with `0` and `true` with `1`, violating the no-type-coercion rule (`forge/spec/query-dsl.md:38`). Add type guards for equality/`IN` the same way range predicates already do, or bind booleans in a representation that cannot collide with numbers.

4. **P2 - Unsupported P1 text/join queries can return bogus rows.** The parser records unsupported `text`/`join` fields (`forge/crates/storage/src/query.rs:221`), but `Store::query` ignores that flag and scans anyway (`forge/crates/storage/src/lib.rs:403`). `QueryResult` also has no warning field despite the fixture/spec allowing `unsupported_feature` during P1 rollout (`forge/crates/storage/src/query.rs:644`, `forge/spec/query-dsl.md:106`). Please return a typed warning/error before planning unsupported queries, and make the fixture tests execute through `Store::query`, not just `Query::from_fixture_value`.

5. **P2 - Descending sort puts nulls first and ignores explicit id desc.** The spec requires nulls last (`forge/spec/query-dsl.md:104`), but `finalize_rows` reverses the full primary comparison for descending order (`forge/crates/storage/src/query.rs:715`, `forge/crates/storage/src/query.rs:723`), which also reverses the null rank. Separately, `orderBy("id", "desc")` is treated as `Ordering::Equal` for the primary key (`forge/crates/storage/src/query.rs:718`), so only the ascending tie-break remains. Keep null ranking independent of direction and handle `id`/`entity_id` as real sortable keys.

6. **P2 - SQL-like validation is a keyword filter, not the committed subset.** `reject_raw_sql` rejects semicolons/comments and a few DDL/DML substrings (`forge/crates/storage/src/query.rs:847`), but the spec also bans subqueries, CTEs, arbitrary functions, wildcard table names, and unbound params unless parsed into the supported AST (`forge/spec/query-dsl.md:72`). Replace the string filter with a parser-to-AST path, or reject SQL-like strings entirely until the validated subset exists.

## Verification

- `git show --check 0e84bf9e` passed.
- `cargo test --locked -p forge-storage` passed on the committed tree before later unrelated edits to `forge/crates/storage/src/query.rs`.
- `cargo clippy --locked -p forge-storage --all-targets -- -D warnings` could not be cleanly attributed to this commit because the current worktree now has an in-progress, uncommitted `query.rs` FieldRef edit that fails compilation.
