# Commit Review: 4569112f storage field-id preservation

Reviewed commit: `4569112f forge-storage: preserve schema-minted field_ids on mutation (review 049)`

## Findings

No new findings. The patch changes mutation materialization to layer `f_<name>` stand-ins on top of existing `field_ids` instead of rebuilding the map, and adds a regression that seeds a schema-minted `f_alice_0` FTS index, patches an unrelated display field, and proves the stable id and FTS visibility survive.

## Notes

- This closes the review 049 concern I raised about display-name mutations clobbering schema-stable field ids. The M0a stale-stand-in tradeoff is documented in the helper and is preferable to dropping schema IDs without a schema name-to-id map.

## Verification

- `git show --check 4569112f`
- `(cd forge && cargo test --locked -p forge-storage mutation_preserves_existing_schema_field_ids_and_index_visibility)`
- `(cd forge && cargo test --locked -p forge-storage)`
- `(cd forge && cargo clippy --locked -p forge-storage --all-targets -- -D warnings)`
