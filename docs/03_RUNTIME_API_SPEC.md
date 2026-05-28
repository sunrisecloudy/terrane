# Runtime API Spec

This document defines the JS surface the runtime exposes to generated apps and the bridge contract every host implements. Section tags **[v0.1]**/**[v0.3]**/**[v0.4]** mark the milestone in which a requirement first appeared.

## 1. Generated app API **[v0.1]**

Generated apps may use exactly this global:

```js
await AppRuntime.call(method, params)
```

Optional event subscription:

```js
const unsubscribe = AppRuntime.on(eventName, handler)
```

Generated apps must not call native platform APIs directly. The runtime must not place any other globals in the sandbox.

### 1.1 `AppRuntime.on` event surface **[v0.1]**

The runtime emits a fixed event list. Unknown event names are ignored.

| Event | Payload | When |
|---|---|---|
| `runtime.ready` | `{ runtimeVersion, appId, capabilities }` | After mount, before first frame paint |
| `runtime.suspend` | `{ reason }` (`"background"`, `"locked"`, `"host"`) | Host signals lifecycle suspend |
| `runtime.resume` | `{}` | Host signals lifecycle resume |
| `app.error` | `{ code, message, source }` | Runtime caught an error in the app frame |
| `app.budget_warning` | `{ budget, current, max }` | Resource budget crossed 80% threshold (v0.3) |
| `app.permission_revoked` | `{ permission, reason }` | User or runtime revoked a permission post-install (v0.3) |

Subscriptions are scoped to the calling app's sandbox. Unsubscribing returns void.

## 2. Runtime call contract **[v0.1]**

```ts
type RuntimeCall = (method: string, params: unknown) => Promise<unknown>;
```

All calls must resolve with a result or reject with an error object that matches §5. The implementation must normalize all native errors into the bridge error shape.

### 2.1 App-id derivation **[v0.1, security-critical]**

The runtime must derive the calling `appId` from the sandbox channel, **not** from any field in the request body. Required mechanism:

1. The runtime creates each app iframe and assigns a per-mount nonce `mount_token` (cryptographically random, ≥ 16 bytes, base64url-encoded).
2. The runtime opens a `MessageChannel` and transfers exactly one port to the iframe via `postMessage` before any app script runs. The other port is held by the parent runtime.
3. The parent runtime keeps a `Map<port, { appId, mountToken }>` so that every inbound message is identified by the receiving port, never by a property of the message.
4. If the iframe ever calls the bridge through any path other than its assigned port (for example by trying `window.parent.postMessage`), the runtime rejects the message and emits `app.error` with code `bridge.unauthorized_channel`.
5. Native bridges (WKScriptMessageHandler / WebMessageListener / WebView2 / WebKitGTK) attach the same `(appId, mountToken)` pair on the host side before dispatch. The native bridge verifies origin/frame in addition to the runtime check.

Generated apps may not request, override, or even read `appId` directly. Reads must happen via `runtime.capabilities` if needed.

## 3. Allowed bridge methods **[v0.1 unless noted]**

### `core.step`

Runs one deterministic event through the Zig core.

Request:

```json
{
  "app": "task-workbench",
  "event": {
    "type": "CreateTask",
    "payload": { "title": "Write spec" }
  }
}
```

Response:

```json
{
  "stateVersion": 3,
  "actions": [
    { "type": "StorageSet", "key": "task-workbench:tasks", "value": [] },
    { "type": "Toast", "message": "Task created" }
  ]
}
```

The `app` field is informational. Authority comes from the channel (§2.1); the runtime rejects calls where `app` is set and does not match the channel-derived appId.

### `storage.get` / `storage.set` / `storage.remove` / `storage.list`

See docs/27 §9 for the database mapping. All keys must begin with the manifest `storagePrefix`; the runtime rejects mismatches with `permission_denied`.

```json
// storage.get  request
{ "key": "notes-lite:notes", "defaultValue": [] }
// storage.get  response
{ "value": [] }

// storage.set  request
{ "key": "notes-lite:notes", "value": [] }
// storage.set  response
{ "ok": true, "bytesWritten": 2 }

// storage.remove  request
{ "key": "notes-lite:notes" }
// storage.remove  response
{ "ok": true }

// storage.list  request
{ "prefix": "notes-lite:" }
// storage.list  response
{ "keys": ["notes-lite:notes"] }
```

### `dialog.openFile` / `dialog.saveFile`

```json
// dialog.openFile  request
{ "accept": ["text/plain", "application/json"], "multiple": false, "maxBytes": 1048576 }
// dialog.openFile  response
{
  "files": [
    { "name": "sample.txt", "mime": "text/plain", "size": 123, "text": "hello" }
  ]
}

// dialog.saveFile  request
{ "suggestedName": "output.txt", "mime": "text/plain", "text": "hello" }
// dialog.saveFile  response
{ "ok": true }
```

Cancelled dialogs return error `dialog_cancelled`. Binary support arrives with v0.5 `assets/` work.

### `notification.toast`

```json
// request
{ "message": "Saved", "level": "success" }
// response
{ "ok": true }
```

`level` must be one of `info`, `success`, `warning`, `error`.

### `network.request`

```json
// request
{
  "url": "https://example.com/api/status",
  "method": "GET",
  "headers": {},
  "body": null,
  "timeoutMs": 10000
}
// response
{ "status": 200, "headers": {}, "bodyText": "{}" }
```

The host must enforce `manifest.networkPolicy` (docs/24). Disallowed origins, methods, or headers return `network_policy_denied`. Redirects to disallowed origins are rejected. Generated apps must not use direct `fetch`.

### `app.log`

```json
// request
{ "level": "info", "message": "Loaded app", "data": {} }
// response
{ "ok": true }
```

`level` is one of `debug`, `info`, `warn`, `error`. The runtime is permitted (but not required) to suppress `debug` in production.

### `runtime.capabilities` **[v0.3]**

```json
// request
{}
// response
{ "platform": "macos", "runtimeVersion": "0.1.0", "features": { "dialog.saveFile": true, "network.request": true, "snapshot": false }, "appId": "notes-lite" }
```

Response validates against `schemas/runtime-capabilities.schema.json`. See §9.

## 4. Permission mapping **[v0.1]**

| Method | Required permission |
|---|---|
| `core.step` | `core.step` |
| `storage.get` | `storage.read` |
| `storage.set` | `storage.write` |
| `storage.remove` | `storage.write` |
| `storage.list` | `storage.read` |
| `dialog.openFile` | `dialog.openFile` |
| `dialog.saveFile` | `dialog.saveFile` |
| `notification.toast` | `notification.toast` |
| `network.request` | `network.request` |
| `app.log` | none (always allowed; subject to rate budget) |
| `runtime.capabilities` | none (always allowed; v0.3) |

`app.log` is intentionally permission-less so apps can always emit diagnostic logs. It is still rate-limited by `resourceBudget.maxLogLinesPerMinute`.

## 5. Bridge errors **[v0.1]**

All errors must use this shape:

```json
{ "code": "permission_denied", "message": "Human-readable explanation", "details": {} }
```

Canonical error codes:

- `invalid_request`
- `unknown_method`
- `permission_denied`
- `quota_exceeded`
- `RESOURCE_BUDGET_EXCEEDED` (v0.3)
- `runtime_version_incompatible` (v0.1; see docs/04 §8)
- `bridge.unauthorized_channel` (v0.1; see §2.1)
- `storage_error`
- `network_error`
- `network_policy_denied` (v0.3)
- `dialog_cancelled`
- `core_error`
- `platform_unsupported`
- `capability_unavailable` (v0.3)
- `timeout`
- `invalid_response`

## 6. Runtime internal modules **[v0.1]**

### PermissionManager

Inputs: app manifest, method name, params.
Output: allow/deny with reason.

### QuotaManager

Tracks: bridge calls per second, storage bytes per app, max open dialogs, network requests per minute, max generated app DOM nodes.

### SandboxManager

Responsibilities: create iframe, generate `mount_token`, set up MessageChannel, inject `AppRuntime`, route messages, kill/reload app, collect errors.

### RegistryManager

Responsibilities: install app package, update app package, remove app package, list installed apps, validate package compatibility.

## 7. Local development mock **[v0.1]**

The runtime supports a browser-only mock host for development:

```js
window.__APP_RUNTIME_DEV_MOCK__ = true;
```

The mock may emulate storage, `core.step`, and network responses. It must never be used as a production security boundary. The fake host (docs/32) is the durable reference; this in-page mock exists only for fast loops while editing runtime code.

## 8. Capabilities API canonical form **[v0.3]**

There is exactly one form for capability discovery: `AppRuntime.call("runtime.capabilities", {})`. A thin convenience wrapper `AppRuntime.capabilities()` is permitted in the runtime; it delegates to the bridge method and is not a separate code path.

```js
const caps = await AppRuntime.call("runtime.capabilities", {});
// equivalent to:
const caps = await AppRuntime.capabilities();
```

The response must validate against `schemas/runtime-capabilities.schema.json`. Apps must use this to gate optional features (e.g., `dialog.saveFile`).

## 9. Installed-package verification before mount **[v0.3]**

Before mounting a generated app the runtime must verify, in order:

1. Package signature and content hashes (docs/17).
2. App version status is `enabled` (or `dev-installed` in dev builds).
3. `runtimeVersion` is compatible per docs/04 §8.
4. Required capabilities are available on this platform.
5. User-approved permissions match manifest permissions.
6. Resource budgets are active and within platform clamps.
7. Network policy is loaded into the network bridge.

Any failure produces a structured error and refuses mount; the previous active version (if any) is kept active.

## 10. Dev snapshot and resource APIs **[v0.3]**

In dev mode, the runtime and control plane must support the snapshot format defined in `schemas/runtime-snapshot.schema.json` and expose resource usage:

```text
runtime.resource_usage   -> { domNodes, storageBytes, bridgeCalls, networkCalls, timers, logCount, packageBytes }
```

These are dev-only paths; they are never reachable from generated apps.

## 11. Storage persistence mapping **[v0.4]**

The generated app API remains build-free and SQL-free. `storage.*` calls map to platform database operations:

| Runtime method | Permission | Database operation |
|---|---|---|
| `storage.get` | `storage.read` | `SELECT value_json FROM app_storage WHERE app_id=? AND key=?` |
| `storage.set` | `storage.write` | `INSERT OR REPLACE INTO app_storage(app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)` |
| `storage.list` | `storage.read` | `SELECT key FROM app_storage WHERE app_id=? AND key LIKE prefix` |
| `storage.remove` | `storage.write` | `DELETE FROM app_storage WHERE app_id=? AND key=?` |

The runtime derives `app_id` from the channel (§2.1) and rejects keys that do not start with the manifest `storagePrefix`.

## 12. Codex-only database inspection **[v0.4]**

Generated apps cannot call DB inspection methods. Codex may call these through the dev control plane only:

```text
db.snapshot
db.query_app_storage
db.query_app_versions
db.query_bridge_calls
db.query_core_events
db.query_test_runs
db.export_debug_bundle
```

The production runtime must not expose these to generated apps. Arbitrary SQL is disabled by default and only permitted under an explicit unsafe dev-mode flag (`runtime.unsafe_sql`) on the fake host.

## 13. Numbering and history

| Section | First added |
|---|---|
| 1, 2, 3, 4, 5, 6, 7 | v0.1 |
| 1.1, 2.1 | v0.1 (added in v0.4 revision) |
| 8, 9, 10 | v0.3 |
| 11, 12 | v0.4 |
