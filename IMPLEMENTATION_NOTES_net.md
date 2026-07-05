# Implementation Notes: net v2

## Files changed

- `rust/crates/terrane-cap-interface/src/abi.rs`: added `Effect::HttpRequest`.
- `rust/crates/terrane-cap-net/src/request.rs`: request parsing, validation, canonical JSON, redaction, `$secret` reservation, request key hashing, and body/redirect/timeout modes.
- `rust/crates/terrane-cap-net/src/lib.rs`: additive `net.request` and `net.call`, `net.responded`, `NetState.requests`, deterministic fold/describe, and app removal cleanup.
- `rust/crates/terrane-cap-net/src/doc.rs`: net v2 command/resource/event docs, limits, redaction, `$secret`, blob body, and SSRF notes.
- `rust/crates/terrane-cap-net/tests/capability.rs`: integration coverage for command decisions, canonicalization, redaction, `$secret`, fold, cleanup, and docs.
- `rust/crates/terrane-cap-net/src/tests.rs`: removed old inline tests per capability test rule; coverage now lives in `tests/capability.rs`.
- `rust/crates/terrane-host/src/edge.rs`: edge `HttpRequest` runner, redirect policies, metadata-address denial, response header filtering, inline/blob body selection, CAS insert, and `blob.stored` link.
- `rust/crates/terrane-host/src/cli.rs`: help entry for `net request` and state display for recorded request responses.
- `rust/crates/terrane-host/src/public_authz.rs`: `net.request` is grant-gated by the existing `net` namespace.
- `rust/crates/terrane-core/tests/cap/net.rs`: deterministic core coverage for `net.responded` replay identity and transient `ctx.resource.net.call`.
- `rust/crates/terrane-host/tests/cap/net.rs`: loopback e2e coverage for POST/headers/body, redaction, replay, redirect policies, timeout, and blob offload.
- `rust/crates/terrane-host/tests/public_authz.rs`: inventory and grant-gated public authz coverage for `net.request`.
- `docs/APP_API.md`: app-facing `ctx.resource.net.call(request_json)` and recorded `net.request` usage notes.
- `rust/crates/terrane-cap-net/Cargo.toml`, `Cargo.lock`: added `base64`, `serde_json`, and `sha2` dependencies.

## Key design choices

- `net.fetch` and `net.get` remain backward-compatible and keep folding `net.fetched`.
- `net.request` records only the redacted canonical request JSON plus the observed response. Replay folds `net.responded`; it never re-sends HTTP.
- Built-in sensitive headers and app-declared `sensitiveHeaders` redact before `EventRecord` construction.
- `{"$secret":"name"}` validates and round-trips in canonical JSON, but the edge rejects unresolved secrets until the future oauth/secret resolver exists.
- Response bodies use inline text for small text responses, blob CAS for binary/large/forced blob responses, and `blob.stored` under `__net__/<request_key>` when offloaded.
- Redirect handling is explicit in the edge runner so `manual`, `deny`, five-hop `follow`, per-hop SSRF validation, and HTTPS-to-HTTP downgrade refusal stay under Terrane control.

## Deviations

- No secret store or substitution was implemented; unresolved `$secret` values intentionally error at the edge, matching the locked decision.
- Private and loopback addresses remain allowed except for `169.254.169.254`, matching the local-first SSRF rule.

## Shared files touched

- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/tests/cap/net.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/net.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Proof tests

- Happy path and validation: `net_request_canonicalizes_redacts_and_folds_recorded_response`, `net_request_posts_redacts_and_replays_on_loopback`.
- Replay identity: `responded_event_folds_redacted_request_and_replays_identically`, `net_request_posts_redacts_and_replays_on_loopback`.
- Transient resource behavior: `net_call_resource_returns_inline_body_but_records_nothing`.
- Security and limits: `net_request_reserves_secret_tokens_without_resolving_them`, `net_request_posts_redacts_and_replays_on_loopback`, `net_request_timeout_errors_on_loopback`.
- Redirect policy: `net_request_redirect_policies_are_enforced_on_loopback`.
- Blob offload: `net_request_offloads_binary_response_to_blob`.
- Public authorization: `net_request_is_grant_gated_for_public_callers`.

## Gate

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
