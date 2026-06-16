# Review 116: core determinism extraction

Reviewed commit `9c8d4a7e` (`forge-core: extract determinism (seed/run-id) into determinism.rs`).

## Findings

- No blocking findings. `run_seed_override`, `seed_field`, `derive_seeds`, `fnv1a64`, and `unique_run_id` moved into `forge/crates/core/src/determinism.rs`; `workspace.rs` imports them unchanged, and the seed override tests moved with the helpers.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
