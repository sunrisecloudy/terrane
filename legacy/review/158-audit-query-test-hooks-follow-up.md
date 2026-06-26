# Review 158 - audit.query self-audit + test hook gating follow-up

Reviewed commit `01fb2a22` (`forge-core: require audit.query self-audit (review 156) + gate simulate_failure_stage behind test-hooks (review 157)`).

No actionable findings.

Checks performed:

- `cargo test -p forge-core a_failed_self_audit_append_fails_the_read_rather_than_returning_rows_unlogged --test audit_query_command` passes.
- `cargo tree -p forge-cli -i forge-core -e features` shows `forge-cli` depends on `forge-core` with only the `default` feature, so the `test-hooks` fault injection gate is absent from the CLI dependency graph.
- `rg "simulate_failure_stage" forge/crates/core/src ...` found no remaining production command-path reads outside `commands::test_hooks`.
