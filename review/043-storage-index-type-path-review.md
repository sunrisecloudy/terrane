# Commit Review: 4287ef5e storage type/path fixes

Reviewed commit: `4287ef5e forge-storage: fix indexes type/path bugs (DL-5/DL-16)`

## Findings

No new findings in this commit. The boolean comparison guard and quoted stable-field-id JSON paths line up with the earlier review feedback around type coercion and dotted field IDs, and the added tests cover query, value-index, and FTS behavior for those cases.

Do not treat this as closing the broader dynamic-index work yet. The still-actionable issues from `review/042-storage-index-lifecycle-review.md` remain open unless addressed separately:

- Core mutation paths still bypass FTS sync, so ordinary DL-17 mutations can leave FTS indexes stale.
- Value and FTS index definitions still collide on `(collection, field_id)`.
- `text_search` still bypasses the normal filter/sort/limit/group pipeline.
- The planner still misses sort-only and mixed-filter fallback warnings.

## Verification

- `git show --check 4287ef5e` passed.
- `cargo test --locked -p forge-storage` passed.
- `cargo clippy --locked -p forge-storage --all-targets -- -D warnings` passed.
