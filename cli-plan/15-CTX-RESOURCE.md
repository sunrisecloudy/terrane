# 15 — `ctx.resource` (camera first, extensible)

Plan for a single extensible host namespace for platform capabilities (camera,
microphone, clipboard, …) without adding a new top-level `ctx.*` per kind.

**Status:** implemented (runtime + mock + tests; native shells return `PlatformUnavailable` until wired).

**Related:** prd-merged/01 CR-3 (platform capabilities), `forge/spec/policy-gates.md`
(platform-permission gate), `cli-plan/14-EFFECT-SURFACE-AND-OBSERVABILITY.md`
(inner door + `RecordedCall` journal).

---

## Problem

CR-3 lists platform capabilities (`camera`, `microphone`, `clipboard`, …).
Policy already models `PlatformPermissions`, but there is no applet-facing
`ctx` namespace and no replay path. A naive design would return captured image
bytes inline from `invoke`:

```json
{ "bytes_base64": "..." }
```

That is the wrong default: it forces a large base64 string through QuickJS,
JSON serialization, and the `RecordedCall.response` field on every capture —
slow, memory-heavy, and bad for trace UIs. **Camera success must be
handle-based; bytes stay on the host until explicitly requested.**

---

## API shape

### Namespace

```ts
ctx.resource.invoke(kind: string, args?: JsonValue[]): Promise<ResourceInvokeResult>
ctx.resource.read(asset_id: string, request?: ResourceReadRequest): Promise<ResourceReadResponse>
ctx.resource.materialize(
  asset_id: string,
  request: ResourceMaterializeRequest
): Promise<ResourceMaterializeResponse>
```

- **`invoke`** — open/trigger a platform resource (camera shutter, future mic
  start, clipboard read, …). Returns **metadata + `asset_id` only**.
- **`read`** — lazy byte retrieval when the applet must process bytes in-script
  (optional path; still crosses the JS boundary as base64).
- **`materialize`** — host copies the blob into the `ctx.files` sandbox **without**
  the applet handling raw bytes (preferred for “attach photo to note”).

All three are host-bridge methods, capability-checked, counted against
`max_host_calls`, and recorded in `RunRecord` like `net.fetch` and `files.read`.

### Camera `invoke` args (optional `args[0]` object)

| Field | Type | Purpose |
| --- | --- | --- |
| `facing` | `"front" \| "back" \| "environment"` | Camera selection hint |
| `max_bytes` | number | Cap captured payload (enforced before accept) |
| `content_type` | string | Expected MIME (e.g. `image/jpeg`) |
| `max_dimension` | number | Long-edge pixel cap (host may downscale) |

### Camera `invoke` success (locked — no inline bytes)

```json
{
  "asset_id": "res_cam_01HXYZ…",
  "content_type": "image/jpeg",
  "width": 1920,
  "height": 1080,
  "size_bytes": 245760
}
```

The host stores the raw blob in a **run-scoped resource store** keyed by
`asset_id`. The `RecordedCall` for `resource.invoke` records this metadata
JSON only. Blobs live in a parallel `RunRecord` sidecar (see Replay).

### `read` response (when needed)

```json
{
  "asset_id": "res_cam_01HXYZ…",
  "bytes_base64": "…",
  "size_bytes": 245760,
  "content_type": "image/jpeg"
}
```

Base64 here is acceptable as an **opt-in, second host-call** after the app
already holds a small handle. Chunked `read` (`offset` / `limit`) is a later
extension if large assets need it.

### `materialize` request / response

Request:

```json
{
  "handle": "workspace_data",
  "path": "attachments/photo-01.jpg"
}
```

Response:

```json
{
  "asset_id": "res_cam_01HXYZ…",
  "handle": "workspace_data",
  "path": "attachments/photo-01.jpg",
  "written_bytes": 245760,
  "content_type": "image/jpeg"
}
```

Gates: manifest must grant `capabilities.resources` (camera) **and**
`capabilities.files.write` for the target handle/path glob. The host performs
the copy natively; JS never sees the blob.

### Errors

| Code | When |
| --- | --- |
| `CapabilityRequired` | Manifest lacks `resources: ["camera"]` (or kind) |
| `PermissionDenied` | Policy / role gate failed |
| `PlatformUnavailable` | Host OS has not granted camera (or no hardware) |
| `resource_cancelled` | User dismissed the capture UI |
| `ResourceLimitExceeded` | `max_bytes` / `max_dimension` / host-call budget |

Typed as `CoreError` variants consistent with `files` and `net` surfaces.

---

## Manifest

v1 string list under `capabilities.resources`:

```json
{
  "capabilities": {
    "resources": ["camera"]
  }
}
```

Later: object rules per kind (quality caps, facing default, etc.) without
changing the `invoke(kind, args)` call shape.

---

## Policy

Extend `forge-policy`:

- `HostCall::Resource { kind, args }` for `invoke`
- `HostCall::ResourceRead { asset_id }` / `ResourceMaterialize { asset_id, handle, path }`
- `Category::Resource` mapped to platform permission for known kinds:
  `camera` → OS camera permission (SC-10 platform-permission gate)

Unknown `kind` → `CapabilityRequired` or `ValidationError` before bridge touch.

---

## Runtime

### Bridge

Add to `HostBridge` (`forge/crates/runtime/src/bridge.rs`):

- `resource_invoke(kind, args) -> ResourceInvokeResult`
- `resource_read(asset_id, request) -> ResourceReadResponse`
- `resource_materialize(asset_id, request) -> ResourceMaterializeResponse`

### Provider trait

`ResourceProvider` — native shells implement camera capture; tests use
`MockResourceProvider` (fixed JPEG bytes, deterministic dimensions).

`HostContext` (`forge/crates/runtime/src/host/`) gates each call, records
`RecordedCall`, and on live runs stores blobs in `ResourceStore` (in-memory map
for tests; native blob dir scoped to run/workspace in production).

### Engine

Wire `ctx.resource.*` in QuickJS install path (`engine.rs`), mirroring
`ctx.files` / `ctx.net.fetch` error mapping and validation.

---

## Replay & determinism

```
invoke("camera")  →  RecordedCall { method: "resource.invoke", response: { asset_id, … } }
                     + RunRecord.resource_assets[asset_id] = { bytes, content_type, … }

read(asset_id)    →  RecordedCall serves bytes from resource_assets (no live camera)

materialize(…)    →  RecordedCall + files side effects from recorded blob (same as live)
```

- **`RunRecord` extension:** `resource_assets: BTreeMap<String, ResourceAssetBlob>`
  (canonical JSON; bytes as base64 **in the run record file**, not in JS invoke
  response). Replay never opens the live camera.
- **Mismatch** between live call sequence and recording → determinism violation
  (same as `net.fetch` / `files.read`).
- **`system.trace`:** show `asset_id` + metadata for `invoke`; omit or truncate
  blob bodies in trace UI (configurable cap, e.g. first 256 bytes).

---

## Phases

| Phase | Deliverable |
| --- | --- |
| **A — Spec & types** | `forge/spec/resources.md`, `Capabilities.resources`, `forge-std.d.ts`, policy `HostCall` / `Category::Resource` |
| **B — Runtime & mock** | `ResourceProvider`, bridge + host gates, `MockResourceProvider`, recorder/replay with `resource_assets` sidecar |
| **C — Example e2e** | Extend `notes-lite` (or bundled fixture): capture → `materialize` → attach path in DB; conformance vectors under `forge/fixtures/resources/` |
| **D — Native shells** | macOS/iOS/Android camera bridge (others return `PlatformUnavailable`) |
| **E — Docs & contract** | Public API docs, `system.describe` inner entries, contract export, agent-adapter projector |

---

## Example applet flow (notes-lite)

```ts
const shot = await ctx.resource.invoke("camera", [{ facing: "back", max_bytes: 512_000 }]);
await ctx.resource.materialize(shot.asset_id, {
  handle: "workspace_data",
  path: `attachments/${shot.asset_id}.jpg`,
});
await ctx.db.insert("notes", {
  title: input.title,
  attachment_path: `attachments/${shot.asset_id}.jpg`,
  attachment_meta: { width: shot.width, height: shot.height },
});
```

No base64 in the hot path.

---

## Open decisions

| ID | Question | Recommendation |
| --- | --- | --- |
| R1 | Ship `read`, `materialize`, or both in v1? | **Locked: both** — `materialize` is the documented happy path; `read` for in-script byte processing |
| R2 | `asset_id` format | ULID-style `res_{kind}_{id}` for trace readability |
| R3 | Blob retention after run | Scoped to run record + workspace blob store; GC policy in storage PRD later |
| R4 | Seventh bundled app vs extend notes-lite | Extend **notes-lite** to avoid `bundled-apps.json` parity churn |

---

## Files to touch (implementation checklist)

| Area | Path |
| --- | --- |
| Spec | `forge/spec/resources.md` (new) |
| Manifest | `forge/crates/domain/src/manifest.rs` |
| Run record | `forge/crates/domain/src/run.rs` (`resource_assets`) |
| Policy | `forge/crates/policy/src/lib.rs` |
| Bridge | `forge/crates/runtime/src/bridge.rs`, `host/resource.rs` (new) |
| Engine | `forge/crates/runtime/src/engine.rs` |
| Types | `forge/std/forge-std.d.ts` |
| Fixtures | `forge/fixtures/resources/*.json` |
| Docs | `forge/docs/public-api-reference.md`, generator |
| Example | `webapps/examples/notes-lite/` |

---

## Non-goals (v1)

- Video / burst / live preview streams
- Microphone, clipboard, location (same `invoke` shape later; not v1)
- Inline `bytes_base64` on `invoke` success (explicitly rejected)