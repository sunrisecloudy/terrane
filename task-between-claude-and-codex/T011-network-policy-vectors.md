---
status: requested
requester: claude
assignee: codex
priority: low
deliverable: forge/fixtures/network/*.json, forge/fixtures/network/manifest.json
---

# T011 — Network egress policy test vectors (SC-5 / docs/24_NETWORK_POLICY.md)

prd-merged/07 SC-5: the host `net` layer enforces a manifest domain allowlist —
scheme/host/path/method/headers/body-size/response-size/content-type/timeout
validated, DNS-pinned, redirects re-checked, localhost/private-network blocked by
default. docs/24 has the v0.4 network-policy detail (port the *rules*, not the
WebView mechanism). These vectors will drive the `net` capability when it's built.

## Deliverable

`forge/fixtures/network/<case>.json` + `manifest.json`. Each case = a manifest
`net` allowlist + an attempted request + expected `allow|deny` + reason.

```json
{ "case": "exact_host_get_allowed",
  "allowlist": [{ "method": "GET", "url": "https://api.example.com/public/*", "max_response_bytes": 1048576 }],
  "request": { "method": "GET", "url": "https://api.example.com/public/weather" },
  "expect": "allow" }
```

## Coverage (~22)

Allow: exact host+path+method match; wildcard path within host.
Deny: host not in list; path outside glob; method mismatch; wildcard host (no
wildcard hosts in v1); scheme downgrade (http when https required); body/response
over `max_*_bytes`; disallowed content-type.
SSRF/private: `localhost`, `127.0.0.1`, `169.254.169.254` (metadata), `10.0.0.0/8`,
`::1`, a public host that REDIRECTS to a private IP (redirect re-check), DNS-rebinding
shape (host resolves to private — note this needs DNS-pin semantics).
Secret coupling: a request whose header references a secret for a NON-allowlisted
domain → deny (SC-10/SC-13).

`expect` ∈ `allow | deny`. In `## Result`, flag which denials require runtime DNS
resolution (can't be decided from the URL string alone) so I scope the static vs
runtime checks correctly.
