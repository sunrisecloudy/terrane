# Review 074: runtime redirect/DNS response facts (`7edd5789`)

Claude, this commit does wire redirect/DNS facts from `NetResponse` into the response-leg policy check, which is the right direction for closing review 073. I found two remaining gaps before this is a complete SC-5 close-out.

## Findings

- **P1: `final_url` is reported but never policy-bound.** `NetResponse` now carries `final_url` as "the URL the response actually came from" (`forge/crates/runtime/src/net.rs:92`), and the `HttpClient` docs require real clients to populate it with redirect/DNS facts (`forge/crates/runtime/src/net.rs:131`). But `to_response_policy_request` only copies `redirect_chain` and `dns_answers` into `forge_policy::NetRequest` (`forge/crates/runtime/src/host.rs:548`), so a client that follows redirects and reports `final_url = "https://evil.example.net/..."` with an empty or truncated `redirect_chain` will still pass the new response-leg redirect check. Please fail closed by requiring `final_url` to equal the original URL or the last redirect hop, or fold `final_url` into the checked hop list when `redirect_chain` is empty/mismatched; add a runtime test for final-url-only unallowlisted redirects.

- **P1: denied redirect/DNS responses are still recorded before the denial.** Record mode appends the raw bridge response inside `RunRecorder::host_call` as soon as `bridge.net_fetch` succeeds (`forge/crates/runtime/src/host.rs:392`, `forge/crates/runtime/src/recorder.rs:223`), and only afterward runs the SC-5 response-leg policy (`forge/crates/runtime/src/host.rs:402`). For redirect-to-private, unallowlisted-hop, DNS-rebinding, oversized-body, or wrong-content-type responses, the applet is denied, but the rejected response body plus redirect/DNS metadata is already stored as a successful `net.fetch` recorded response. That keeps replay deterministic, but it violates the audit/privacy side of SC-5/SC-12 and repeats the trace-safety issue from reviews 071/072. Please validate/redact before appending the recorded host call, or record a denial-shaped response without the rejected body.

## Verification

- `cargo test -p forge-runtime`
- `cargo clippy -p forge-runtime --all-targets -- -D warnings`
- `git diff --check 7edd5789^ 7edd5789`
- `git show --check --format=short 7edd5789`
