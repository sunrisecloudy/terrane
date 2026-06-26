# Review 077 - commits fa36b0e7 + b33571f6 ctx.secrets

## Findings

1. **P1 - Response-leg-denied `net.fetch` recordings no longer replay as the same denial.** The new redaction path rewrites a captured response-leg violation into the denial shape `{"denied": <CoreError>}` (`forge/crates/runtime/src/recorder.rs:271-287`). On replay, `HostContext::net_fetch` still consumes that recorded response via `host_call`, then immediately deserializes it as a `NetResponse` before any policy-denial reconstruction (`forge/crates/runtime/src/host.rs:429-441`). A redacted response has no `status`, so replay reports `RuntimeError("net.fetch response decode failed...")` instead of the original `PermissionDenied`, and the recorded run is not byte-identical. Please detect the denial-shaped response before `NetResponse` decode (or store a replayable redacted `NetResponse` plus error metadata) and add a replay test for `redirect_after_secret_injection_denied_trace_safe`.

2. **P2 - Runtime/core bypass the new `forge-secrets` crate and duplicate its contract.** `prd-merged/01-core-runtime-prd.md:18-25` separates `runtime/` from `secrets/`, and commit `be2b68d5` added `forge-secrets` as the keychain/keystore abstraction in the workspace (`forge/Cargo.toml:28-34`). These new commits instead define another `SecretStore`, `InMemorySecretStore`, and `resolve_secret_headers` inside `forge-runtime` (`forge/crates/runtime/src/net.rs:264-368`), then wire core to `forge_runtime::SecretStore` (`forge/crates/core/src/workspace.rs:112-132`). The original `forge-secrets` trait/resolver remains unused by non-test code (`forge/crates/secrets/src/lib.rs:93-105`, `forge/crates/secrets/src/lib.rs:168-201`), so shell authors now have two incompatible secret-store APIs and the redacting `SecretValue`/backend-error behavior from `forge-secrets` does not protect the production path. Please make `forge-runtime`/`forge-core` depend on `forge-secrets` (or remove the crate intentionally) before more SC-13 work lands.

## Verification

- `cargo test -p forge-runtime`
- `cargo clippy -p forge-runtime --all-targets -- -D warnings`
- `cargo check -p forge-runtime --target wasm32-unknown-unknown`
- `cargo test -p forge-core`
- `cargo clippy -p forge-core --all-targets -- -D warnings`
- `git diff --check fa36b0e7^ b33571f6`
- `git show --check --format=short fa36b0e7`
- `git show --check --format=short b33571f6`
