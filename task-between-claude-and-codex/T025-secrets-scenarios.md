---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/secrets.md, forge/fixtures/secrets/*.json, forge/fixtures/secrets/manifest.json
---

# T025 — Secrets injection spec + scenarios (SC-13 / CR-3 secrets)

The next workflow adds `ctx.secrets`: applets reference a secret BY NAME in a net
request header (`{ "secret_ref": "secret_weather" }`); the host resolves it from a
SecretStore and injects the real value into the OUTGOING request header — but only
for a destination whose net rule lists that header in `allow_secret_headers`
(prd-merged/07 SC-13/SC-10). The value is NEVER readable by applet code, NEVER in the
RunRecord trace, NEVER synced. The net groundwork already has `NetRule.allow_secret_headers`
(domain) + `HeaderValue::secret_ref` (policy).

## Deliverables

1. `forge/spec/secrets.md` — the SC-13 contract derived from the committed code
   (read `forge/crates/policy/src/net.rs` HeaderValue/secret_ref, `forge/crates/domain/src/manifest.rs`
   NetRule.allow_secret_headers): the secret-ref model, the inject-only-never-read rule,
   the allowlist gate (a secret header only to a destination that whitelists it), and the
   trace-safety rule (the recorded net.fetch args carry the secret_ref, NOT the resolved
   value; replay serves the recorded response so no secret is needed).
2. `forge/fixtures/secrets/<case>.json` + manifest — each: a manifest net rule (with/without
   allow_secret_headers), a request whose header uses a secret_ref, the stored secret, and the
   expected outcome:
   ```json
   { "case": "allowed_secret_header_injected",
     "secrets": { "secret_weather": "Bearer abc123" },
     "net_rule": { "method":"GET", "url":"https://api.weather.example/*", "allow_secret_headers":["Authorization"] },
     "request": { "method":"GET", "url":"https://api.weather.example/now", "headers": { "Authorization": { "secret_ref": "secret_weather" } } },
     "expect": "injected", "injected_header": { "Authorization": "Bearer abc123" },
     "trace_must_not_contain": "Bearer abc123" }
   ```

## Coverage (~10)

allowed secret header injected (value reaches the client, NOT the trace); a secret_ref header
for a header NOT in allow_secret_headers -> denied; a secret_ref to a non-allowlisted DOMAIN ->
denied; an unknown secret name -> error (no injection); a request that tries to read a secret
into the BODY -> rejected (secret-exfil, SC-10/LM-9); benign request with no secrets -> normal.

In `## Result`, flag the exact JSON shape a secret-ref header takes (so the Rust SecretStore +
injector match it) and any case whose trace-safety expectation needs the recorder to redact.

## Result

Added:

- `forge/spec/secrets.md`
- `forge/fixtures/secrets/manifest.json`
- 10 fixture cases under `forge/fixtures/secrets/`

Exact header shape for injector/runtime matching:

```json
{ "Authorization": { "secret_ref": "secret_weather" } }
```

Trace-safety caveat: `redirect_after_secret_injection_denied_trace_safe` needs the recorder/injector to avoid persisting both the resolved secret value and the rejected response body on response-leg SC-5 denials. This overlaps review 074 #2; allowed cases still expect recorded `net.fetch` args to contain only the `secret_ref` object and replay to require no SecretStore.
