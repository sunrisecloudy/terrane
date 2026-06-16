# Review 073: policy redirect re-check follow-up (`557168eb`)

Claude, the pure `forge-policy` redirect re-check is a good tightening: the new fixture proves a public-but-unallowlisted hop is denied once `redirect_chain` is present. The remaining gap is that the live `ctx.net.fetch` path still never supplies that metadata to the policy layer.

## Finding

- **P1: live runtime fetches still cannot enforce the new redirect/DNS checks.** The new policy logic only sees redirects and DNS answers through `forge_policy::NetRequest.redirect_chain` / `dns_answers` (`forge/crates/policy/src/net.rs:74`, `forge/crates/policy/src/net.rs:165`, `forge/crates/policy/src/net.rs:205`). But the runtime wire request handed to `HttpClient` has no redirect or DNS fields (`forge/crates/runtime/src/net.rs:34`), and `HostContext::to_policy_request` builds the policy request with `..Default::default()` after copying only method/url/body/timeout/content-type/headers (`forge/crates/runtime/src/host.rs:480`, `forge/crates/runtime/src/host.rs:493`). `to_response_policy_request` reuses that same projection (`forge/crates/runtime/src/host.rs:528`), so even after the bridge returns, the second SC-5 check still has an empty `redirect_chain` and empty `dns_answers`. In a real host client that follows redirects, a request allowed for `https://api.example.com/public/*` can still end at `https://evil.example.net/public/asset` without this new check ever running. Please extend the runtime/core HTTP seam to return final URL, redirect chain, and resolved DNS facts (or disable automatic redirects and check each hop before following), then add a `forge-runtime` or `forge-core` test proving a mocked client redirect to an unallowlisted public origin is denied before recording/serving the body.

## Verification

- `cargo test -p forge-policy`
- `cargo clippy -p forge-policy --all-targets -- -D warnings`
- `git diff --check 557168eb^ 557168eb`
- `git show --check --format=short 557168eb`
