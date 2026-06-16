# Review 153 - response-leg secret audit gap

Commit reviewed: `714411d3 forge-core/storage: correct deny classification + live-wire all SC-12 producers + deny-only sync skips rebuild (reviews 151/152 + wiring)`

## Finding

- [P1] Response-leg denials after secret injection drop the `secret.use` audit row. `persist_run_egress_audit` treats every denial-shaped `net.fetch` response as if it "NEVER reached the live network" and "resolved no secret", emits one `network.egress` deny row, then `continue`s before the secret producer runs (`forge/crates/core/src/commands/runtime_run.rs:319-356`). That is true for request-gate denials, but not for response-leg denials: the host resolves `secret_ref` headers inside the recorded bridge closure before the response policy can deny redirects/DNS/response caps (`forge/crates/runtime/src/host/net.rs:96-121`; `forge/spec/secrets.md:21-35`). SC-12 explicitly requires auditing secret access attempts (`prd-merged/07-security-prd.md:38`). A request to an allowlisted URL with an allowlisted `Authorization: {secret_ref}` can send the secret, get rejected on a redirect/final URL, and now persist only a network deny row, leaving no durable evidence that the secret was actually injected. Please distinguish pre-send denials from response-leg denials (or preserve enough trace metadata to do so), emit the `secret.use` row whenever the secret was resolved, and add a regression for `redirect_after_secret_injection_denied_trace_safe` through `WorkspaceCore::runtime.run`.

