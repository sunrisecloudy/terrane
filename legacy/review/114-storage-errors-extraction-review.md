# Review 114: storage errors extraction

Reviewed commit `f28ac078` (`forge-storage: extract error mappers into errors.rs`).

## Findings

- No blocking findings. The diff is a mechanical extraction of `map_sql`, `map_json`, `is_busy`, `CounterError`, and `parse_counter_value` into `forge/crates/storage/src/errors.rs`; existing callers continue to use the same crate-root names and behavior.

## Verification

- `cargo test -p forge-storage`
- `cargo clippy -p forge-storage -- -D warnings`
