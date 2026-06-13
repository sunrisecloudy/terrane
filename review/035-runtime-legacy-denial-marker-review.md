# Commit Review: 224e40b8

Reviewed commit: `224e40b8 forge-runtime: close review 029 (legacy failed replay) + confirm seed-threading API for 032`

## Findings

1. **P2 - Avoid treating ordinary user JSON as a recorded denial.** The new legacy fallback gate disables the manifest fallback whenever any recorded response object contains a `denied` key (`forge/crates/runtime/src/runner.rs:163`, `:254`). That is not a unique denial marker: `ctx.storage.get`, `ctx.db.get`, and `ctx.db.list` replay arbitrary user data (`forge/crates/runtime/src/host.rs:139`, `:232`, `:246`; `forge/crates/core/src/bridge.rs:107`, `:162`, `:171`), so a legitimate snapshotless legacy run that reads a stored value like `{ "denied": false }` and then fails for an app reason will again route through the all-deny default snapshot and replay as a permission failure instead of consuming the recorded response. `record_denial()` writes a very specific shape, `{"denied": <CoreError JSON>}` (`forge/crates/runtime/src/recorder.rs:247`), so please tighten `trace_has_denial` to validate the nested `CoreError` shape/code, or change the denial encoding to an unambiguous wrapper that cannot collide with app data. Add a regression for a snapshotless legacy failure after an allowed `storage.get`/`db.get` response containing a non-error `denied` field.

## Verification

- `git show --check 224e40b8`
- `cargo test --locked -p forge-runtime`
- `cargo clippy --locked -p forge-runtime --all-targets -- -D warnings`
