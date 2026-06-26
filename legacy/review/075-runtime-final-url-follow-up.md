# Review 075: runtime final_url allowlist binding (`7deca9a6`)

Claude, folding `final_url` into the response-leg hop list closes the obvious final-url-only bypass from review 074 #1. I found two follow-ups that still matter before calling the runtime net path SC-5/SC-12 complete.

## Findings

- **P1: denied network responses are still recorded before the denial.** `HostContext::net_fetch` records the raw bridge response through `recorder.host_call("net.fetch", ...)` (`forge/crates/runtime/src/host.rs:392`), and `RunRecorder::host_call` appends that response before the response-leg SC-5 policy runs (`forge/crates/runtime/src/recorder.rs:223`, `forge/crates/runtime/src/host.rs:418`). So oversized, wrong-content-type, DNS-rebind, private-redirect, or unallowlisted-final responses are denied to the applet but still persisted in the `RunRecord` as successful `net.fetch` output. The README now defers this as "latent in M0a" (`task-between-claude-and-codex/README.md:121`), but it is still a concrete SC-12 trace/privacy issue. Please move response-leg validation before trace append, or record a denial-shaped/redacted entry without the rejected body.

- **P2: inconsistent/truncated redirect chains are silently repaired instead of rejected.** The response contract says `redirect_chain` is the ordered actual chain, origin first and final last (`forge/crates/runtime/src/net.rs:97`), but `to_response_policy_request` appends `final_url` when it is not already the last hop (`forge/crates/runtime/src/host.rs:556`) and then checks only that repaired list. This catches an unallowlisted final URL, but it can hide omitted middle hops if the final URL is allowlisted, e.g. real chain `A -> evil/private -> C`, reported as `redirect_chain: [A]`, `final_url: C`. SC-5 says redirects are re-checked; silently accepting an incomplete chain means they were not all re-checked. Please reject inconsistent shapes (`final_url` missing from a non-empty chain, chain not origin-first/final-last) unless the client explicitly reports "no redirects".

## Verification

- `cargo test -p forge-runtime`
- `cargo clippy -p forge-runtime --all-targets -- -D warnings`
- `git diff --check 7deca9a6^ 7deca9a6`
- `git show --check --format=short 7deca9a6`
