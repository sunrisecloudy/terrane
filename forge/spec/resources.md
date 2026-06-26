# Platform Resources (`ctx.resource`)

Source of record: `prd-merged/01-core-runtime-prd.md` CR-3, `prd-merged/07-security-prd.md`
SC-10 platform-permission gate, `cli-plan/15-CTX-RESOURCE.md`.

Extensible namespace for platform capabilities (`camera` first; `microphone`,
`clipboard`, … later) without adding a new top-level `ctx.*` per kind.

## Model

- **`invoke`** returns metadata + `asset_id` only — never inline `bytes_base64`.
- Blobs live in a **run-scoped store** and in `RunRecord.resource_assets` for replay.
- **`read`** — lazy base64 retrieval when the applet must process bytes in-script.
- **`materialize`** — host copies the blob into `ctx.files` (preferred for attachments).

## Manifest

```json
{
  "capabilities": {
    "resources": ["camera"]
  }
}
```

`materialize` additionally requires a matching `capabilities.files.write` grant.

## Host calls

```ts
ctx.resource.invoke(kind: string, args?: JsonValue[]): Promise<ResourceInvokeResult>
ctx.resource.read(asset_id: string, request?: ResourceReadRequest): Promise<ResourceReadResponse>
ctx.resource.materialize(asset_id: string, request: ResourceMaterializeRequest): Promise<ResourceMaterializeResponse>
```

### Camera `invoke` success (no inline bytes)

```json
{
  "asset_id": "res_camera_0",
  "content_type": "image/jpeg",
  "width": 640,
  "height": 480,
  "size_bytes": 22
}
```

### Camera `invoke` args (optional `args[0]`)

| Field | Type | Purpose |
| --- | --- | --- |
| `facing` | string | `front` / `back` / `environment` hint |
| `max_bytes` | number | Cap captured payload |
| `content_type` | string | Expected MIME |
| `max_dimension` | number | Long-edge pixel cap (future) |

## Gates

Every call passes:

1. Actor role may run applets (SC-10).
2. Manifest lists the resource kind under `capabilities.resources`.
3. Platform-permission gate (`Category::Resource`) for live runs.
4. Host-call budget (`max_host_calls`).
5. Record/replay (`RecordedCall` + `resource_assets` sidecar).

`materialize` also runs the full `ctx.files.write` gate chain.

## Errors

| Code | When |
| --- | --- |
| `CapabilityRequired` | No `capabilities.resources` grant |
| `PermissionDenied` | Kind not listed / files grant missing |
| `PlatformUnavailable` | OS has not granted camera |
| `RuntimeError` (`resource_cancelled`) | User dismissed capture UI |
| `ResourceLimitExceeded` | `max_bytes` / host-call budget |

## Replay

```
invoke  → RecordedCall { method: "resource.invoke", response: metadata }
        + RunRecord.resource_assets[asset_id]

read    → serves bytes from resource_assets (no live camera)

materialize → RecordedCall + recorded files.write (same bytes on replay)
```