# Review 076 - commit be2b68d5 forge-secrets

## Findings

1. **P1 - `secret_ref` cannot reach the recorder-safe runtime path yet.** The new crate documents the intended SC-13 behavior as if it is live: recorded `net.fetch` args keep the `{ "secret_ref": ... }` object and the host resolves the value inside the `host_call` closure (`forge/crates/secrets/src/lib.rs:13-20`). Current runtime still deserializes request headers as `BTreeMap<String, String>` (`forge/crates/runtime/src/net.rs:33-42`), so the documented object shape fails validation at the JS boundary (`forge/crates/runtime/src/engine.rs:699-709`). Even if a secret header were constructed internally, `to_policy_request` maps every runtime header back to `HeaderValue::Literal` (`forge/crates/runtime/src/host.rs:484-499`), so `HeaderValue::Secret` never reaches policy. The host also records `args` before the bridge call (`forge/crates/runtime/src/host.rs:352-393`). If the next integration resolves secrets before building this runtime `NetRequest`, plaintext headers will be serialized into the run record. Please wire runtime headers through `forge_policy::HeaderValue`, record the pre-injection request, and resolve only inside the live bridge closure before marking SC-13 injection complete.

2. **P2 - `resolve_secret_headers` loses the matched-rule binding.** SC-13 says values are injected only for allowlisted domains (`prd-merged/07-security-prd.md:40-42`), and the local secrets spec requires the target net rule to match the destination/method before its `allow_secret_headers` is honored (`forge/spec/secrets.md:27-35`). The policy enforces that binding while matching a specific rule/host (`forge/crates/policy/src/net.rs:392-405`), but the new resolver accepts only a bare `allow_secret_headers: &[String]` and headers (`forge/crates/secrets/src/lib.rs:197-229`). That makes future runtime wiring easy to misuse by passing an allowlist from the wrong rule. Prefer returning/accepting a typed matched net rule (or equivalent policy decision token) and add a two-rule test where only a different host permits `Authorization`.

3. **P3 - `SecretStore::get` docs disagree with resolver behavior.** The trait docs say `Ok(None)` becomes `PermissionDenied` (`forge/crates/secrets/src/lib.rs:93-97`), but `resolve_secret_headers` returns `RuntimeError` for a missing secret (`forge/crates/secrets/src/lib.rs:224-228`), matching the tests and the T025 `unknown_secret_name_error` fixture. The behavior looks right; please update the stale trait comment so future callers do not build assertions around the wrong error class.

## Verification

- `cargo test -p forge-secrets`
- `cargo clippy -p forge-secrets --all-targets -- -D warnings`
- `cargo check -p forge-secrets --target wasm32-unknown-unknown`
- `git diff --check be2b68d5^ be2b68d5`
- `git show --check --format=short be2b68d5`
