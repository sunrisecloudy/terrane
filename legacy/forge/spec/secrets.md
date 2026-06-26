# Secrets Injection Scenarios

Source of record: `prd-merged/07-security-prd.md` SC-13/SC-12, `prd-merged/01-core-runtime-prd.md` CR-3/CR-8, `prd-merged/04-llm-system-prd.md` LM-9. Current code anchors: `forge_domain::NetRule.allow_secret_headers` and `forge_policy::net::HeaderValue::Secret`.

This document pins the initial `ctx.secrets` / `ctx.net.fetch` secret-injection contract before the Rust `SecretStore` lands. The current committed policy layer already understands secret-bearing header refs; the runtime injector is still planned.

## Model

Secrets are stored outside the workspace database by name or ref in a platform secret store. Applet code never receives the plaintext value. A net request may reference a secret in a header value using this exact JSON shape:

```json
{
  "headers": {
    "Authorization": {
      "secret_ref": "secret_weather"
    }
  }
}
```

The host resolves `secret_weather` only at the HTTP edge, injects the resolved value into the outgoing request header, and keeps the original request trace as the `secret_ref` object. Literal secret-like headers such as `Authorization: "Bearer abc"` are rejected by policy.

M0a only defines header injection. Secret injection into query params or request bodies is out of scope until the capability grammar explicitly grows a target shape for it.

## Gates

Every secret injection must pass all of these gates:

- The applet can only name a secret ref; it cannot read, list, or stringify the value.
- The target net rule must match the request destination and method.
- The target net rule must list the header name in `allow_secret_headers`.
- Header name comparison is case-insensitive, matching existing net policy behavior.
- Missing or revoked secret refs fail before any request reaches the client.
- Secret refs in request bodies are rejected as secret-exfil patterns, not injected.
- Redirect and DNS SC-5 checks still apply after the transport returns; a denied response must not expose the secret value in trace or logs.

## Trace And Replay

Run records store the pre-injection request:

```json
{
  "method": "net.fetch",
  "args": [
    {
      "method": "GET",
      "url": "https://api.weather.example/now",
      "headers": {
        "Authorization": {
          "secret_ref": "secret_weather"
        }
      }
    }
  ]
}
```

The resolved value, for example `Bearer abc123`, is never recorded, logged, synced, exported, or sent to the LLM context builder. Replay serves the recorded host response and therefore does not require the secret store.

## Result

Fixture shape for a secret-ref header is exactly:

```json
{ "secret_ref": "secret_weather" }
```

Trace-safety cases that need recorder/injector care:

- `allowed_secret_header_injected`, `allowed_secret_header_case_insensitive`, and `trace_redacts_injected_secret_value` require the outgoing client request to contain the resolved value while the recorded `net.fetch` args keep only `{ "secret_ref": "..." }`.
- `redirect_after_secret_injection_denied_trace_safe` requires the eventual denied response path to avoid recording rejected response bodies or resolved secret values.
- Existing review 074 #2 still applies: response-leg denials must be recorded as denial-shaped/redacted entries before real HTTP clients can be considered trace-safe.
