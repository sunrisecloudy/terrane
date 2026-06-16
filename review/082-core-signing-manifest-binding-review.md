# Review 082 - core signing payload binding follow-up

Reviewed commit: `b01a6e74 forge-core: app signing/trust (SC-15) — bind signed package to install payload (review 080 #1)`.

## Findings

1. **P1 - Signed installs still do not bind the signed manifest/policy to the manifest that is stored and enforced.** The new `bind_signature_to_sources` check only compares signed `files` to install `sources`, while `cmd_applet_install` still validates and later stores the separate top-level `manifest` (`forge/crates/core/src/workspace.rs:429`, `forge/crates/core/src/workspace.rs:449`, `forge/crates/core/src/workspace.rs:482`, `forge/crates/core/src/workspace.rs:1956`, `forge/crates/core/src/workspace.rs:1988`). The updated positive test now demonstrates the remaining gap: it installs `demo_manifest()` with `storage app/*` and `db tasks` grants while the signed fixture manifest is `app.notes` with `notes/*` / `notes` grants (`forge/crates/core/tests/spine.rs:35`, `forge/crates/core/tests/spine.rs:40`, `forge/crates/core/tests/spine.rs:2077`, `forge/fixtures/signing/valid_signature.json:5`, `forge/fixtures/signing/valid_signature.json:12`, `forge/fixtures/signing/valid_signature.json:26`). This means a valid signature over code can still be installed as `Signed` under a different runtime policy, app identity, entrypoint, and resource limits, so the publisher's signed capability boundary is not what the runtime enforces. Fix by deriving the stored `Manifest` from the verified package manifest (after converting to the forge-domain manifest shape) or rejecting unless the install manifest/policy/app id/entrypoint/resource limits exactly match the signed package manifest; add a regression where identical source with broader top-level capabilities is rejected.

## Verification

- `git show --check --format=short b01a6e74` passed.
- `git diff --check cc97f3cd..b01a6e74` passed.
- `cargo test -p forge-core` passed.
- `cargo clippy -p forge-core --all-targets -- -D warnings` passed.
