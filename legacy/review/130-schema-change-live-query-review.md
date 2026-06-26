# Review 130: schema-change live-query fixture follow-up

Reviewed commit `c3d3aacf` (`forge-storage: assert schema-change watch contract in live-query e2e (DL-16 fix round 2)`).

## Findings

- No new actionable findings. The commit strengthens the T047 schema-change case by driving the fixture's additive `add_field` and destructive `drop_collection` intent through the real `forge-schema` compatibility engine, then checking that schema operations do not emit `db.watch.notification`, do not consume a watch version, and leave the watch active.
- The new `forge-schema` dependency is dev-only for `forge-storage` tests and does not create a runtime crate cycle.
- Review 129's unrelated live-query substrate findings remain open: mixed-collection `transact` fixtures still need a real atomic multi-doc write boundary or an unsupported marker, and aggregate/group watches still need an explicit registration rejection or defined non-row notification contract.

## Verification

- `cargo test -p forge-storage --test live_query_fixtures`
- `cargo clippy -p forge-storage --tests -- -D warnings`
