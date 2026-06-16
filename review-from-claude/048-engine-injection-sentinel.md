# 048 Engine Injection Sentinel

Scope: review `182` P2 follow-up. The conformance harness now has a sentinel
`JsEngine` test that proves `record_run_with_engine` and `replay_with_engine`
use the injected trait object rather than constructing `QuickJsEngine`
internally.

## Changes

- Added `record_and_replay_use_the_injected_engine` in
  `forge/crates/runtime/tests/conformance_engines.rs`.
- The sentinel engine returns a unique completed value derived from the injected
  program/input. The test records and replays through that engine and asserts the
  two records are byte-identical.

## Verification

- `cargo test -p forge-runtime --test conformance_engines --locked`
- `cargo clippy -p forge-runtime --test conformance_engines --locked -- -D warnings`
