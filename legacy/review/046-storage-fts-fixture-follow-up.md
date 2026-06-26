# Commit Review: c9f7630f FTS filter fixture follow-up

Reviewed commit: `c9f7630f forge-storage: close review 040/041/042 (strengthen FTS filter+limit fixture to be mutation-discriminating) +134 tests`

## Findings

- **[P2] The strengthened fixture still locks in the missing full-scan warning.** The updated vector now makes the `tag=data` filter load-bearing for row correctness: `note_1` is `tag=personal` (`forge/fixtures/indexes/fts_text_search_with_filter_and_limit.json:43`) and the expected row is `note_4` (`:142`). That is good. But the same query still filters on unindexed display field `tag` (`:118`) and expects `warnings: []` (`:139`), while `forge/spec/dynamic-indexes.md:139` requires `planner.full_scan` whenever the planner scans `records` for an uncovered predicate/sort/text search. Because `IndexManager::plan()` returns immediately for `text_search` before evaluating filter/sort coverage, this fixture now proves the result pipeline but still masks the planner-warning bug from review 045 #2. Please expect a `no_index` warning for `tag`, or add a separate indexed-tag case if the no-warning behavior is intentional.

- **[P2] This fixture does not exercise the DL-17 mutation path or the `field_ids` issue.** The index fixture harness seeds envelopes by deserializing `records[]` and calling `store.put_record()` (`forge/crates/storage/tests/index_fixtures.rs:38`), and this fixture hand-populates `field_ids` in the seed data (`forge/fixtures/indexes/fts_text_search_with_filter_and_limit.json:47`). That bypasses the mutation code path called out in review 045 #1, where `Mutation::Insert` builds `RecordEnvelope::new(...)` and therefore starts with empty `field_ids`. If the goal is to make the FTS/index coverage mutation-discriminating, add a test that inserts or patches through `apply_mutation` / `transact_mutations`, then asserts both stored `field_ids` and active FTS/index visibility without a manual seed/rebuild.

## Verification

- `git show --check c9f7630f` passed.
- `cargo test --locked -p forge-storage fts_text_search_with_filter_and_limit` passed.
- `cargo test --locked -p forge-storage mutation_insert_patch_delete` passed.
