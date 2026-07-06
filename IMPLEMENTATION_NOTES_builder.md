# Implementation Notes: builder bundle smoke tests

## Files changed

- `rust/crates/terrane-host/src/lib.rs`
  - Added optional `tests.json` parsing and validation helpers.
  - Runs smoke cases through `run_js_bundle` with the same temporary resource-grant pattern used by the common API probes.
- `rust/crates/terrane-host/src/edge.rs`
  - Runs smoke tests for generated harness drafts before recording `builder.generated`.
  - Runs smoke tests for `app.import` in-memory bundle files.
- `rust/crates/terrane-host/src/mcp.rs`
  - Extends staged `app_build_validate` / inline bundle inspection to include optional smoke tests.
- `rust/crates/terrane-host/src/mcp_tests.rs`
  - Added staged-builder validation tests for passing and failing `tests.json`.
- `rust/crates/terrane-host/tests/cap/app.rs`
  - Added binary-level install/import validation tests for passing and failing `tests.json`.
- `docs/APP_API.md`
  - Documented the optional `tests.json` format.

## Design choices

- `tests.json` is fully optional. Missing or empty files are a no-op.
- Cases are `{ "verb": "...", "args": ["..."], "expect": ... }`.
- `expect` supports:
  - string shorthand or `{ "contains": "..." }` for substring checks;
  - `{ "jsonSubset": ... }` for recursive JSON subset matching;
  - `{ "shape": ... }` for JSON shape checks with `string`, `number`, `boolean`, `null`, `array`, `object`, or `any`.
- Validation lives at the host edge because it executes JS. No new replay state, central command enum, or capability event format was added.
- Smoke-test failures are surfaced as clear validation errors such as `tests.json case 1 (echo) failed expectation: ...`.

## Deviations

- No new `terrane-cap-builder` command was added. The existing builder cap remains draft/event state only; JS execution stays in `terrane-host`, consistent with the existing common API probe path.

## Shared files touched

- `docs/APP_API.md`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/mcp.rs`
- `rust/crates/terrane-host/src/mcp_tests.rs`
- `rust/crates/terrane-host/tests/cap/app.rs`

## Proof

- `mcp::tests::app_build_validate_runs_passing_tests_json`
- `mcp::tests::app_build_validate_rejects_failing_tests_json`
- `app::app_install_runs_optional_bundle_smoke_tests`
- `app::app_install_rejects_failing_bundle_smoke_test`

Gate:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help
```
