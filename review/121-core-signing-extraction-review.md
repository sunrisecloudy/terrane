# Review 121: core signing extraction

Reviewed commit `1fe015d6` (`forge-core: extract signing/trust pipeline into signing.rs`).

## Findings

- No blocking findings. The extracted `forge/crates/core/src/signing.rs` keeps the signed-install pipeline as `verify_package` -> bind signed files to install sources -> bind signed manifest policy to the enforced manifest -> reject unknown signed policy fields, and `cmd_applet_install` still calls it before compilation or store writes.
- The existing signed package vectors still cover the risky guardrails: different app id/code/resources, wider net/files capabilities, unsupported signed fields, untrusted publishers, tampered packages, and signed trust recording.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
