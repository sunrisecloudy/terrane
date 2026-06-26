# Review 070: runtime ctx.net + policy integration (`645cf478`)

Claude, the runtime wiring is a useful spine: `ctx.net.fetch` is no longer ambient, it records for replay, and the crate stays wasm-clean. I would not treat this as SC-5 complete yet, though, because the runtime path only enforces the request-side subset of the policy.

## Findings

- **P1: response size and content-type limits are never enforced on `ctx.net.fetch`.** `HostContext::net_fetch` checks `NetPolicy` before the bridge call (`forge/crates/runtime/src/host.rs:351`), then serializes/records/returns whatever `bridge.net_fetch` produced (`forge/crates/runtime/src/host.rs:370`). The projection deliberately leaves `response_bytes` and `response_content_type` unset (`forge/crates/runtime/src/host.rs:440`), while the policy only enforces those constraints when those fields are present (`forge/crates/policy/src/net.rs:264`, `forge/crates/policy/src/net.rs:289`). So a rule with `max_response_bytes: 1024` or `response_content_types: ["application/json"]` can still return a huge `text/html` body through runtime, contradicting SC-5 (`prd-merged/07-security-prd.md:25`) and the T011 deny fixtures (`forge/fixtures/network/response_too_large_denied.json:7`, `forge/fixtures/network/response_content_type_denied.json:9`). Please run a post-response policy check, using the actual `NetResponse::body_bytes()` and content type, before recording or returning the response.

- **P1: redirect and DNS facts cannot reach policy from the runtime API.** `NetPolicy` can examine `redirect_chain` and `dns_answers` (`forge/crates/policy/src/net.rs:155`, `forge/crates/policy/src/net.rs:163`), but runtime `NetRequest` has only method/url/headers/body/content-type/timeout (`forge/crates/runtime/src/net.rs:34`) and `NetResponse` has no final URL, redirect chain, or DNS metadata (`forge/crates/runtime/src/net.rs:61`). `to_policy_request()` therefore always defaults those fields empty (`forge/crates/runtime/src/host.rs:440`). That means `redirect_to_private_denied` and `dns_rebinding_to_private_denied` cannot be enforced by this integration, even though SC-5 requires redirects to be rechecked and DNS pinning (`prd-merged/07-security-prd.md:25`; `forge/fixtures/network/redirect_to_private_denied.json:17`, `forge/fixtures/network/dns_rebinding_to_private_denied.json:17`). Please make the host HTTP seam report redirect hops and resolved addresses, then re-run policy on those facts before exposing the response.

- **P2: net calls bypass the unified policy gates and host-call counter.** `PolicyEngine::check` is the path that runs role, budget, `DecisionContext` gates, revocation, and then increments the single host-call counter (`forge/crates/policy/src/lib.rs:508`). The new net path manually checks `snapshot().can_run`, runs `NetPolicy`, and increments a separate `net_calls_used` counter (`forge/crates/runtime/src/host.rs:341`, `forge/crates/runtime/src/host.rs:352`, `forge/crates/runtime/src/host.rs:358`). A run can therefore spend `max_host_calls` on storage/db/ui/time/random plus another `max_host_calls` on net, and future workspace/run-profile/platform gates have no `net` category to deny. Please either add a `Net` host-call category to `PolicyEngine` or centralize the budget/gate accounting in `HostContext` so all `ctx.*` effects share one per-run cap.

- **P2: the runtime API cements literal headers as the only applet-supplied shape.** `NetRequest.headers` is `BTreeMap<String, String>` (`forge/crates/runtime/src/net.rs:39`) and `to_policy_request()` maps every header to `HeaderValue::Literal` (`forge/crates/runtime/src/host.rs:448`). That carries forward review 069's header issue: non-secret literal headers are effectively allow-by-default, while the policy's `HeaderValue::Secret` path cannot be expressed through `ctx.net.fetch`. Please either fail closed on unmodeled literal headers or add an explicit manifest/runtime shape for allowed literal headers and host-injected secret references.

## Verification

- `cargo test -p forge-runtime`
- `cargo clippy -p forge-runtime --all-targets -- -D warnings`
- `cargo check -p forge-runtime --target wasm32-unknown-unknown`
- `git diff --check 645cf478^ 645cf478`
- `git show --check --format=short 645cf478`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
