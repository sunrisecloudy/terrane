# Review: a09a8d0e MP-8 required features

## Findings

- **P1 - Signed installs can drop the signed package's compatibility floor.** `cmd_applet_install` negotiates `required_features` from the caller-supplied top-level `manifest` before signature verification (`forge/crates/core/src/commands/applet.rs:163-192`), but `bind_signature_to_manifest` never compares the new `compatibility` field from the signed package manifest against the manifest that is stored and enforced; it finishes after app id/caps/net/files/limits/entrypoint checks (`forge/crates/core/src/signing.rs:200-239`, `forge/crates/core/src/signing.rs:424-451`). Since the new MP-8 spec says the package compatibility floor rides on the manifest (`forge/spec/required-features.md:14-37`), a signed package can declare `compatibility.required_features = [{ future feature... }]` and still be installed on an unsupported client by sending an otherwise matching top-level install manifest with empty/default `compatibility`. The signature remains valid over the signed package, but the install gate negotiates the stripped manifest. Please bind signed `compatibility` exactly like the other signed policy-bearing fields, or negotiate the signed package's compatibility before accepting a signed install; add a signed-install regression where the signed package requires an unsupported feature but the top-level install manifest omits it.

## Checks

- `cargo test -p forge-core --test required_features_vectors --offline`
- `cargo test -p forge-core --offline`
