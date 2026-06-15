# 047 Runtime/Core Forge Secrets Integration

Scope: review `078` P2 / `077` P2 follow-up. `forge-runtime` and `forge-core`
now consume the canonical `forge-secrets` store/value/injector seam instead of a
duplicate runtime-local `SecretStore::resolve` API.

## Changes

- Added `forge-secrets` as a direct `forge-runtime` dependency and re-exported
  `SecretStore`, `SecretValue`, and `InMemorySecretStore` from `forge-runtime`
  for source compatibility.
- Replaced runtime's duplicate secret store implementation with a small adapter
  that converts `NetHeaderValue` to `forge_policy::HeaderValue`, calls
  `forge_secrets::resolve_secret_headers`, then returns a literal-only
  `NetRequest` for the HTTP client.
- Added `NetPolicy::allowed_secret_headers` so `HostContext::net_fetch` can pass
  the matched request-phase rule's `allow_secret_headers` to the injector.
- Made `forge-secrets::InMemorySecretStore` lock-backed so runtime's shared
  empty store remains valid, and preserved the builder-style `with_secret` test
  helper.
- Updated the one core bridge test that still asserted the removed
  `resolve()`-style API.

## Review Notes

- Sidecar subagents audited the integration design and test plan. They confirmed
  the minimal safe path, called out the `OnceLock`/`RefCell` issue, and suggested
  the matched-rule policy test added in this slice.
- External `claude -p` review was not retried for this slice because the sandbox
  reviewer previously rejected sending the private checkout to external Claude.

## Verification

- `cargo test -p forge-secrets -p forge-policy -p forge-runtime -p forge-core --locked`
- `cargo clippy -p forge-secrets -p forge-policy -p forge-runtime -p forge-core --all-targets --locked -- -D warnings`
- `cargo check -p forge-runtime --target wasm32-unknown-unknown --locked`
- `node --no-warnings tools/check-repo.mjs`
- `git diff --check`
