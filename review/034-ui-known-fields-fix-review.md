# Commit Review: cac0e19c

Reviewed commit: `cac0e19c forge-ui: close review 030 (preserve known @forge/std node fields)`

## Findings

No actionable findings. The fix covers the review 030 gap: known `@forge/std` fields from `BaseNode`, `Stack`, `Text`, `Button`, `TextField`, and `List` are now represented in the typed `Node` model, re-emitted during serialization, diffed/applied for scalar updates, and regression-tested. The CLI demo also now prints the previously dropped `testId`, `gap`, and `variant` fields while still replaying identically.

## Verification

- `git show --check cac0e19c`
- `cargo test --locked -p forge-ui`
- `cargo clippy --locked -p forge-ui -- -D warnings`
- `cargo run --locked -p forge-cli -- demo`
