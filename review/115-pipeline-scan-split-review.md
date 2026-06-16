# Review 115: pipeline scan module split

Reviewed commit `871f9493` (`forge-pipeline: split scan.rs into scan/ module, pass order preserved`).

## Findings

- No blocking findings. The static-scan pass order remains `parse`/static-import rejection, alias collection, module-scope binding collection, visitor walk, then text backstop in `forge/crates/pipeline/src/scan/mod.rs`, and the public surface still re-exports `policy_scan`, `enforce_policy`, and `ScanFinding`.

## Verification

- `cargo test -p forge-pipeline`
- `cargo clippy -p forge-pipeline -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
