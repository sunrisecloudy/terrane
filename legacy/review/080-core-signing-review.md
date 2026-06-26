# Review 080 - forge-core signing install path

Reviewed commit: `987fca0b forge-core: app signing/trust (SC-15)`.

## Findings

1. **P1 - A valid signature can bless a different app than the one being installed.** `cmd_applet_install` validates and stores the top-level `manifest` and `sources`, but `verify_install_signature` verifies a separate `signature.package` object and never compares the signed package manifest/files to the actual install payload (`forge/crates/core/src/workspace.rs:424`, `forge/crates/core/src/workspace.rs:431`, `forge/crates/core/src/workspace.rs:433`, `forge/crates/core/src/workspace.rs:477`, `forge/crates/core/src/workspace.rs:1913`). The new test helper already demonstrates the gap: it installs `demo_manifest()` plus `DEMO_TS` while attaching `valid_signature.json`, whose signed package is `app.notes` with a one-line `return { ok: true }` source (`forge/crates/core/tests/spine.rs:16`, `forge/crates/core/tests/spine.rs:2055`, `forge/fixtures/signing/valid_signature.json:5`, `forge/fixtures/signing/valid_signature.json:50`). A caller can attach any valid signed package to arbitrary top-level code and still get `InstallTrust::Signed`. Fix by deriving the signed `Package` from the top-level install payload, or by rejecting unless signed manifest/app id/entrypoint/files/hash set exactly match the payload that will be compiled and stored; add a regression where the signed fixture is valid but the top-level source or manifest is changed.

2. **P2 - The required forge-core clippy gate is red after adding the signing dependency.** `cargo clippy -p forge-core --all-targets -- -D warnings` now fails because the newly linked `forge-signing` crate has an unused `preimage` parameter in `crates/signing/src/verify.rs:121`. The v1 working agreement requires the edited crate's clippy command to be clean before commit, so this should be fixed or the parameter intentionally marked unused before treating SC-15 core wiring as green.

## Verification

- `git show --check --format=short 987fca0b` passed.
- `git diff --check 4ddc4f2c..987fca0b` passed.
- `cargo test -p forge-core` passed.
- `cargo clippy -p forge-core --all-targets -- -D warnings` failed on the unused `preimage` variable above.
