# Query Capability Implementation Notes

## Files changed

- New crate: `rust/crates/terrane-cap-query/`
  - `src/lib.rs`: capability surface, commands, query/resource reads, fold state, materialized-view events.
  - `src/pipeline/mod.rs`: deterministic aggregation pipeline engine, expression operators, JSON ordering/canonicalization, limits.
  - `src/source.rs`: app-scoped source resolver over `kv`, `relational_db`, `view`, and inline docs.
  - `src/jmespath.rs`: `jmespath` crate wrapper.
  - `src/doc.rs`: capability documentation.
  - `tests/capability.rs`: engine/JMESPath integration tests.
- Shared wiring:
  - `Cargo.toml`, `Cargo.lock`: workspace member/dependencies for `terrane-cap-query`, `jmespath`, `sha2`.
  - `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`: state slice and `default_registry` registration.
  - `rust/crates/terrane-cap-interface/src/runtime.rs`: added `QueryValue::Json` for host query JSON results.
  - `rust/crates/terrane-host/src/mcp.rs`: MCP JSON rendering for `QueryValue::Json`.
  - `rust/crates/terrane-host/src/cli.rs`: read-only `terrane query jmespath <app> <sourceJson> <expression>` host adapter and help text.
  - `rust/crates/terrane-host/src/public_authz.rs`, `rust/crates/terrane-host/tests/public_authz.rs`: classify query materialization commands as grant-gated; keep untrusted `capability_query query.jmespath` refused to avoid app-data reads without the resource grant path.
  - `docs/APP_API.md`: regenerated resource surface and added a query worked example.
- Tests:
  - `rust/crates/terrane-core/tests/cap/query.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-core/tests/cap/interface.rs`
  - `rust/crates/terrane-host/tests/cap/query.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`

## Key design choices

- `query.materialize` runs in `decide` over folded state and emits ordinary query-owned events: `query.materialized` plus `query.row.put`.
- Materialized payloads include `def_hash` and `source_cursor`. `source_cursor` is maintained by `QueryState` as a count of broadcast-folded events, so a later reactive engine can compare freshness without changing event shape.
- Views are query-owned state, not KV rows. Fold of `query.materialized` clears existing rows first for snapshot semantics.
- Sources reuse existing public surfaces: `kv` via `terrane-cap-kv` helpers and `table` via `terrane-cap-relational-db` resource reads.
- JMESPath uses the `jmespath` crate behind a tiny wrapper.
- The app resource methods are flat runtime methods: `jmespath`, `pipeline`, `viewGet`, `viewScan`, `viewStat`, `viewList`.

## Deviations / notes

- Public MCP `capability_query query.jmespath` is deliberately not allowlisted because it can read app data and `capability_query` has no grant handshake. The CLI host adapter and `ctx.resource.query.jmespath` are implemented.
- The v1 pipeline implements the planned stage/operator subset broadly, with unsupported stages/operators returning typed `InvalidInput` messages naming the unsupported item.

## Proof tests

- Crate engine/JMESPath:
  - `jmespath_evaluates_json_documents`
  - `pipeline_groups_sorts_and_projects_deterministically`
  - `unsupported_stage_is_a_typed_error`
- Core capability:
  - `define_materialize_and_read_view_round_trip_replays`
  - `rematerialize_replaces_snapshot_rows`
  - `lookup_joins_kv_orders_to_relational_rows`
  - `view_source_composes_with_jmespath_query`
  - `shuffled_kv_insertion_produces_identical_materialized_events`
  - `limit_errors_are_typed_and_named`
  - `app_removed_drops_query_views`
- Host e2e:
  - `query_e2e_js_backend_reads_pipeline_and_jmespath`
  - `query_cli_jmespath_reads_folded_state`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
