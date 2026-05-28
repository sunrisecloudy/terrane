# Security Model

## 1. Threat model

Generated apps may be wrong, malicious, or over-permissive. Treat every generated app as untrusted unless explicitly installed as a reviewed first-party app.

Threats:

- Calling unauthorized native APIs.
- Reading/writing another app's storage.
- Direct network exfiltration (including DNS-prefetch and CSS-resource side channels).
- Infinite loops or excessive bridge calls.
- Dangerous HTML/JS injection in generated content.
- Confused-deputy attacks against the native host through the bridge.
- App Store policy violations from downloaded-and-executed code (docs/00 D1).
- DOM-XSS via untrusted strings flowing into innerHTML/eval-like sinks (mitigated by Trusted Types where supported; see §8.1).
- Iframe injection by a malicious main-app frame in legacy bridge implementations (mitigated by per-mount MessageChannel; see docs/03 §2.1).

## 2. Defense in depth

Generated app security relies on layered controls. No single layer is the boundary.

```text
Manifest permissions
  + package validator (HTML/CSS/JS policy at install time)
  + canonicalize + sign + immutable install (v0.3)
  + iframe/WebView sandbox + CSP
  + per-mount MessageChannel (channel-derived appId)
  + bridge method allowlist
  + native dispatch allowlist (second permission check)
  + storage namespace enforcement
  + network policy (origin/method/header)
  + resource budgets and quotas
  + audit log + quarantine
```

## 3. Permission model **[v0.1]**

Generated apps declare permissions in `manifest.json`.

```json
{
  "permissions": [
    "storage.read",
    "storage.write",
    "core.step",
    "notification.toast"
  ]
}
```

The runtime checks permission before every bridge call. The native bridge re-checks. Both must allow before the request executes.

## 4. Capability model **[v0.1]**

The bridge exposes capabilities, not raw platform APIs.

Bad:

```js
native.readFile('/Users/x/private.txt')
```

Good:

```js
AppRuntime.call('dialog.openFile', { accept: ['text/plain'] })
```

The runtime exposes platform-feature capabilities via `runtime.capabilities` (docs/03 §8).

## 5. Storage isolation **[v0.1]**

Generated app storage is namespaced by app id.

- The runtime derives `appId` from the channel (docs/03 §2.1) — apps cannot impersonate another app.
- The runtime requires `key.startsWith(manifest.storagePrefix)`. Reject mismatches with `permission_denied`.
- The native bridge re-applies the same prefix check.

```json
{
  "code": "permission_denied",
  "message": "Storage key must start with app prefix"
}
```

## 6. Network restrictions **[v0.1, hardened v0.3]**

Generated apps must not use direct `fetch`, `XMLHttpRequest`, WebSocket, or EventSource. Network requests go through:

```js
AppRuntime.call('network.request', ...)
```

The runtime/native host enforces:

- `network.request` permission.
- `manifest.networkPolicy` origin/method/header allow rules (docs/24).
- Max request body size.
- Max response body size.
- Timeout (default 10 s).
- Redirect handling: redirects to disallowed origins are rejected.

## 7. HTML/JS policy checks **[v0.1]**

Reject generated apps containing:

- remote scripts;
- remote stylesheets;
- inline `<script>` blocks;
- inline event handlers (`onclick`, `onerror`, etc.);
- `eval`;
- `new Function`;
- dynamic `import()`;
- service worker registration;
- direct `fetch`;
- `XMLHttpRequest`;
- WebSocket / EventSource;
- localStorage / sessionStorage / IndexedDB / cookies;
- direct use of platform bridge objects (`webkit.messageHandlers`, `chrome.webview`, `Android.*`);
- forms with external action;
- nested iframes;
- `<base>` with non-relative href.

Static scanning is a defense layer, not a complete security boundary. CSP + per-mount channel + bridge allowlist are the actual boundary.

## 8. Content Security Policy **[v0.1]**

Runtime sets a restrictive policy for generated app contexts:

```text
default-src 'none';
script-src 'self';
style-src 'self';
img-src 'self' data: blob:;
font-src 'self';
connect-src 'none';
frame-src 'none';
frame-ancestors 'none';
base-uri 'none';
form-action 'none';
object-src 'none';
require-trusted-types-for 'script';
trusted-types runtime-default;
```

Notes:

- `connect-src 'none'` ensures the iframe cannot reach any network directly — all network goes through the bridge to the native host.
- `style-src 'self'` (not `'unsafe-inline'`). Generated apps must use `<link rel="stylesheet" href="styles.css">`. If a future revision relaxes this, document the threat model change.
- `frame-ancestors 'none'` plus iframe `sandbox` attribute prevents the app from being framed by anything else.
- `require-trusted-types-for 'script'` enforces Trusted Types on DOM sinks (Android WebView / Chromium). See §8.1.
- The `csp` iframe attribute is set to the same policy so a hostile runtime cannot relax it for the iframe.

### 8.1 Trusted Types **[v0.3]**

The runtime ships a Trusted Types policy named `runtime-default` that:

- Sanitizes any value flowing into `innerHTML` / `outerHTML` / `document.write` via DOMPurify-equivalent rules.
- Refuses non-package URLs in `<script src>` / `<link href>` attribute setters.
- Is the only policy allowed by `trusted-types` directive.

WebKit/WKWebView does not implement Trusted Types as of early 2026. The directive is shipped anyway for forward compatibility on iOS/macOS, and the runtime carries a JS-level sanitizer (DOMPurify or equivalent) as the actual enforcement on those platforms. Android WebView and WebView2 enforce Trusted Types natively.

### 8.2 Cross-origin isolation

For desktop hosts that serve the runtime via a virtual host, set:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

This makes `SharedArrayBuffer` and high-resolution timing unavailable to generated apps unless the host explicitly serves resources with `Cross-Origin-Resource-Policy: same-origin`, which it does only for package files.

## 9. Resource quotas **[v0.1 baseline, v0.3 budgets supersede]**

v0.1 quotas (still enforced if `manifest.resourceBudget` is absent):

| Resource | Suggested v0.1 limit |
|---|---:|
| Bridge calls | 30/sec/app |
| Storage bytes | 5 MB/app |
| Network requests | 20/min/app |
| Network response size | 2 MB |
| File open max bytes | 5 MB |
| Timers | 50/app |
| App package size | 2 MB |
| Log entries retained | 500/app |

v0.3 apps declare `manifest.resourceBudget` (docs/22). The runtime clamps app-requested budgets down to platform defaults.

## 10. Audit logging **[v0.1]**

Record per bridge call:

- app id (channel-derived);
- method;
- permission decision;
- request size;
- response size;
- duration;
- error code;
- runtime session id;
- control session id (if applicable).

Do not log sensitive payloads by default. Payload logging is opt-in via dev mode and is redacted by default for snapshot exports (docs/21).

## 11. Native hardening **[v0.1]**

Native bridge dispatch must:

- Parse JSON strictly. Reject extra top-level fields.
- Reject unknown methods.
- Derive `appId` from host-side frame metadata, never from request body.
- Apply permission checks on the native side too.
- Normalize all errors to the docs/03 §5 shape.
- Avoid exposing raw native objects (file handles, file paths, native pointers) to JavaScript.
- Avoid synchronous blocking calls from the WebView thread where possible.
- Verify the calling frame origin against the runtime's internal host before dispatch (Android: via `WebMessageListener` `sourceOrigin`; iOS/macOS: via `WKScriptMessage.frameInfo.securityOrigin`; Windows: WebView2 `Source`; Linux: WebKit frame URI).

## 12. iOS App Store distribution **[v0.3 hardening]**

Per Apple App Store Review Guideline 4.7 (clarified November 2025), HTML5/JavaScript mini-apps are in scope for App Store review.

- App Store builds ship only the 5 first-party reviewed bundled apps.
- Sideloaded / TestFlight builds may install AI-generated packages with the full Codex repair loop.
- Native hosts must not extend or expose native platform APIs to non-bundled apps beyond the methods listed in docs/03 G4 (Guideline 4.7.2).
- Bundled-distribution builds must implement a mini-app index per Guideline 4.7.4 and an age-restriction gate per Guideline 4.7.5.
- The dev control plane and `runtime.unsafe_eval` / `runtime.unsafe_sql` are compiled out of App Store builds.

## 13. v0.3 security additions

### App signing and immutable install

Generated apps must not run directly from AI output. The host must validate, sign, and install an immutable package version before mounting. See docs/17.

### Rollback and quarantine

When a generated app violates policy, exceeds budgets, fails signature checks, or fails post-install tests, the host must quarantine that installed version and preserve the previous working version. See docs/18.

### Network policy

Generated apps cannot call `fetch`, `XMLHttpRequest`, WebSocket, EventSource, remote scripts, or remote styles. All network access must go through `network.request` and match `manifest.networkPolicy`. See docs/24.

### Resource budgets

The runtime must enforce app-level resource budgets from `manifest.resourceBudget`. Severe or repeated budget violations should quarantine the installed app version. See docs/22.

### Snapshot privacy

Dev snapshots may include user data. The snapshot API supports redaction settings, and Codex must not request non-redacted snapshots outside trusted local development.

## 14. Database security model **[v0.4]**

Generated apps never access SQL, database handles, database file paths, or arbitrary query APIs.

Security rules:

- `storage.*` derives `app_id` from sandbox context; the app cannot choose another app id.
- `storage.*` rejects keys outside `manifest.storagePrefix`.
- Permissions are persisted per app version; permission increases require approval (docs/17 §8).
- App installs are transactional; failed validation or migration cannot partially activate an app (docs/27 §6).
- Runtime snapshots and bridge/core logs are debug data and must be excluded from normal backup unless explicitly requested.
- Codex DB inspection is available only through dev-control tools.
- Arbitrary SQL execution is disabled by default and only permitted in explicit unsafe dev mode (`runtime.unsafe_sql`).
- Backup import revalidates packages, permissions, signatures, and runtime compatibility before activation.

## 15. Control plane security **[v0.2]**

See docs/14 §Authentication for the full spec. Summary:

- Bound to `127.0.0.1` in dev builds only.
- Compiled out of production builds.
- Per-launch token written to a file with mode `0600` in the per-user runtime state dir.
- Token rotated on every host start.
- Token required as `X-Platform-Control-Token` header on every request.
- Audit log entry written to `control_commands` for every accepted call, including rejected ones.

## 16. Severity classification

| Class | Examples | Default response |
|---|---|---|
| Boundary breach | unauthorized channel call, prefix mismatch, unknown method | reject + audit; no automatic quarantine |
| Budget violation | bridge/storage/network/timer/dom budget exceeded | 3 in 60 s → quarantine |
| Policy violation | networkPolicy mismatch, signature mismatch at mount | refuse mount; quarantine the version |
| Crash | WebView process crash within 5 s of mount | 3 in 24 h → quarantine |
| Host integrity | DB corruption, signature key error | refuse mount; alert in install report |
