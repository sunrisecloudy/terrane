# Commit Review: 0c101eab storage review 045/046 follow-up

Reviewed commit: `0c101eab forge-storage: close review 045/046 (materialize field_ids in mutation path, planner full_scan on text_search uncovered filter/sort, distinct FTS lifecycle reasons) +4 tests, 137 green`

## Findings

1. **P1 - `apply_mutation` now drops existing schema-stable `field_ids` on update/patch.** `forge/spec/query-dsl.md:93` says `update` preserves `field_ids`, `unknown_fields`, and `extensions` not mentioned by the caller, but this patch rebuilds the entire map from display fields (`f_<name>`) every time (`forge/crates/storage/src/lib.rs:1349`, `forge/crates/storage/src/lib.rs:1463`, `forge/crates/storage/src/lib.rs:1481`). That fixes brand-new DL-17 inserts, but it corrupts records that already carry schema-minted IDs such as `f_alice_0` / `f_dev.01_0`: a display-field patch will replace those IDs with `f_body` / `f_tag`, and active FTS sync deletes the old row before re-reading text from `$.field_ids.<field_id>` (`forge/crates/storage/src/index.rs:462`, `forge/crates/storage/src/index.rs:487`). Any active expression/FTS index keyed to the real schema ID can stop seeing the record after an otherwise unrelated mutation. Please preserve existing stable IDs and only materialize missing display-name stand-ins for M0a, with a regression that seeds a record/index on a schema ID (`f_alice_0`), applies `apply_mutation` patch/update, and proves the original `field_ids` and FTS/index visibility survive.

## Notes

- The planner fixes for text-search filter warnings and distinct non-active FTS lifecycle reasons look covered by unit/fixture tests.

## Verification

- `git show --check 0c101eab`
- `(cd forge && cargo test --locked -p forge-storage fts_text_search_with_filter_and_limit)`
- `(cd forge && cargo test --locked -p forge-storage apply_mutation_keeps_active_fts_in_sync_without_rebuild)`
- `(cd forge && cargo test --locked -p forge-storage mutation_insert_patch_delete)`
- `(cd forge && cargo test --locked -p forge-storage)`
- `(cd forge && cargo clippy --locked -p forge-storage --all-targets -- -D warnings)`
