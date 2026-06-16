# Review 120: core persistence extraction

Reviewed commit `27080847` (`forge-core: extract KV-schema persistence into persistence.rs`).

## Findings

- No blocking findings. The extracted `forge/crates/core/src/persistence.rs` keeps the existing `__forge/meta` key schema for run counters, UI tree diff bases, and applet lifecycle flags, and `workspace.rs` now delegates through thin wrappers without changing the call ordering.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
