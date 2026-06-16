# Commit Review: 11204794 storage review closures

Reviewed commit: `11204794 forge-storage: close review 040/041/042 (...) +134 tests`

## Findings

- **[P1] Mutation vectors still do not verify or produce stable `field_ids`.** The T021 mutation fixture expects the post-state to carry `field_ids` (`forge/fixtures/query/mutation_insert_patch_delete.json:56`), which matters because DL-7 requires stable field ids and DL-5 indexes read `$.field_ids.<id>`. The test deserializes that expected envelope but only compares `fields`, `updated_at`, and `created_at` (`forge/crates/storage/tests/query_fixtures.rs:287`), so it passes while ignoring the expected stable ids. The implementation also builds inserts with `RecordEnvelope::new(...)` (`forge/crates/storage/src/lib.rs:1302`), and that constructor sets `field_ids` to an empty map (`forge/crates/domain/src/record.rs:60`). Net effect: a record inserted through the DL-17 mutation surface will not be visible to expression/FTS indexes keyed by stable field id unless it was pre-seeded another way. Please either wire schema/name→field_id materialization into mutations before FTS/index sync, or make the fixture expectation match reality and add a separate indexed mutation test that proves inserted records populate the indexed stable ids.

- **[P2] `text_search` suppresses full-scan warnings for its `where`/sort pipeline.** `IndexManager::plan()` returns immediately when `query.text_search` is present (`forge/crates/storage/src/index.rs:630`), so the later predicate and sort coverage checks at `forge/crates/storage/src/index.rs:650` and `:670` never run. The new fixture locks this in: it searches an FTS-indexed `body` but filters on unindexed `tag` (`forge/fixtures/indexes/fts_text_search_with_filter_and_limit.json:118`) and expects `warnings: []` (`:139`). That contradicts `forge/spec/dynamic-indexes.md:139`, which requires `planner.full_scan` whenever the planner scans `records` for a predicate, sort, or text search not covered by an active index. Keep the FTS `uses_index`, but also evaluate filter/sort coverage and emit a `no_index` warning for the unindexed `tag` scan.

- **[P3] Non-active FTS definitions lose lifecycle-specific warning reasons.** Expression indexes use `IndexState::full_scan_reason()` to distinguish `index_deprecated` from `index_not_active` (`forge/crates/storage/src/index.rs:127`), but `plan_text_search()` collapses every missing, deprecated, stale, or rebuilding FTS definition into `FtsNotAvailable` (`forge/crates/storage/src/index.rs:757`). That makes a deprecated FTS index indistinguishable from no FTS definition at all, which weakens the DL-5 lifecycle signal this commit is trying to close. Mirror the expression-index path for FTS states and add a deprecated/stale FTS planner test.

## Verification

- `git show --check 11204794` passed.
- `cargo test --locked -p forge-storage` passed.
- `cargo clippy --locked -p forge-storage --all-targets -- -D warnings` passed.
