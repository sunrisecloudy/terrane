# Runtime Capabilities

## 1. Purpose

The same generated app should run on iOS, Android, desktop, fake host, and server test surfaces. Capabilities let an app discover what the current host supports.

## 2. API

The runtime must expose:

```js
const caps = await AppRuntime.capabilities();
```

Response validates against `schemas/runtime-capabilities.schema.json`.

Example:

```json
{
  "runtimeVersion": "0.1.0",
  "platform": "ios",
  "target": "ios-simulator",
  "appId": "notes-lite",
  "devMode": true,
  "features": {
    "core.step": true,
    "storage.read": true,
    "storage.write": true,
    "dialog.openFile": true,
    "dialog.saveFile": false,
    "network.request": true,
    "notification.toast": true,
    "app.log": true,
    "runtime.snapshot": false
  },
  "limits": {
    "maxBodyBytes": 1048576,
    "maxStorageBytes": 5242880,
    "maxBridgeCallsPerMinute": 600
  }
}
```

`appId` is derived from the runtime/native sandbox channel and is informational only. Apps must not put `appId` in bridge request bodies.

`devMode` reflects the host build/runtime mode. Native release builds must report `false`; debug/simulator/dev-control hosts may report `true`.

## 3. Manifest capabilities

Every manifest must include:

```json
{
  "capabilities": {
    "required": ["storage.read", "storage.write"],
    "optional": ["notification.toast"]
  }
}
```

Rules:

- `required` capabilities must be present before mount.
- `optional` capabilities may be absent; app must degrade gracefully.
- Required and optional capability names must not include methods absent from `permissions`, except non-bridge runtime features such as `runtime.darkMode`.

## 4. Platform fallback

If a required capability is missing, runtime shows a platform-compatible error page and does not mount the app.

If an optional capability is missing, runtime returns:

```json
{
  "ok": false,
  "error": {
    "code": "CAPABILITY_UNAVAILABLE",
    "message": "dialog.saveFile is unavailable on this platform"
  }
}
```

## 5. Codex use

Before repairing an app on a platform, Codex should call:

```text
runtime.capabilities
runtime.snapshot
```

Then it should patch app behavior based on actual features rather than assuming desktop parity.
