# Network Policy

## 1. Purpose

Generated apps must not perform direct network access. All network access goes through the host-mediated bridge method:

```js
await AppRuntime.call("network.request", params)
```

The host enforces method, origin, header, body, response-size, and timeout policy.

Use `schemas/network-policy.schema.json`.

## 2. Manifest field

Every manifest must include:

```json
{
  "networkPolicy": {
    "allow": [
      {
        "origin": "https://api.example.com",
        "methods": ["GET"],
        "allowedHeaders": [],
        "maxRequestBytes": 65536,
        "maxResponseBytes": 1048576,
        "timeoutMs": 10000
      }
    ],
    "denyPrivateNetwork": true,
    "allowCredentials": false
  }
}
```

If no network is needed:

```json
{ "networkPolicy": { "allow": [] } }
```

`networkAllowlist` was removed as of v0.4 (docs/00 D6). The package validator must reject manifests that include it. There is no compatibility fallback.

## 3. Request validation

The network bridge must reject:

- URLs not matching the policy;
- methods not allowed by policy;
- headers not allowed by policy;
- request bodies above budget;
- responses above budget;
- redirects to disallowed origins;
- private-network targets when `denyPrivateNetwork` is omitted or `true`;
- cookies and credentialed requests in v0.3 unless explicitly designed.

`denyPrivateNetwork` defaults to `true`. When enabled, hosts reject loopback, link-local, RFC1918, carrier-grade NAT, IPv6 unique-local, and IPv6 link-local literal hosts before applying allow rules. Redirect targets are checked the same way. Apps that genuinely need a local-device endpoint must set `denyPrivateNetwork: false` and still declare exact `allow` rules.

`allowCredentials` defaults to `false` in v0.4, and validators must reject `allowCredentials: true`. Hosts must reject `credentials` request params and `cookie` / `set-cookie` headers until a future credential design is specified.

## 4. Response shape

```json
{
  "status": 200,
  "headers": {
    "content-type": "application/json"
  },
  "bodyText": "{}"
}
```

For binary data, v0.3 should prefer file/dialog workflows. Add binary network response support later if required.

## 5. Codex rules

Codex must not add direct `fetch`, `XMLHttpRequest`, WebSocket, EventSource, or remote scripts. If an app needs new network access, Codex must update `networkPolicy`, permissions, tests, and user-approval requirements.
