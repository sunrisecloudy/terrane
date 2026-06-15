# Zig Core Spec

> **⚠️ SUPERSEDED (2026-06-12):** The Zig core was removed in the Forge cutover. This file is retained only as historical v0.4 reference; current core behavior is implemented in `forge/` and specified by `prd-merged/`.

## 1. Core responsibility

Zig core owns deterministic application logic. It accepts events and returns actions.

It must not directly perform:

- UI rendering.
- Platform storage.
- Network calls.
- File dialogs.
- Notifications.
- WebView operations.
- Mobile lifecycle handling.

## 2. v0.1 FFI API

Header shape:

```c
#ifndef ZIG_CORE_H
#define ZIG_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ZigCore ZigCore;

typedef struct ZigCoreBuffer {
    uint8_t *ptr;
    size_t len;
} ZigCoreBuffer;

ZigCore *core_create(void);
void core_destroy(ZigCore *core);

int32_t core_step_json(
    ZigCore *core,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCoreBuffer *output
);

void core_free(ZigCoreBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
```

## 3. Input shape

```json
{
  "app": "task-workbench",
  "event": {
    "type": "CreateTask",
    "payload": {
      "title": "Write docs"
    }
  },
  "context": {
    "platform": "ios",
    "runtimeVersion": "0.1.0"
  }
}
```

## 4. Output shape

```json
{
  "ok": true,
  "stateVersion": 1,
  "actions": [
    {
      "type": "Toast",
      "message": "Task created",
      "level": "success"
    }
  ]
}
```

Error:

```json
{
  "ok": false,
  "error": {
    "code": "invalid_event",
    "message": "Unknown event type CreateThing"
  },
  "actions": []
}
```

## 5. Core action types in v0.1

- `Toast`
- `StorageSet`
- `StorageRemove`
- `Log`
- `RenderHint`
- `NetworkRequest` for future/native execution demos
- `SaveFile` for file transformer demos

The native/runtime layer may execute these actions or return them to the webapp for display depending on app policy.

## 6. Determinism requirement

For the same initial state and same ordered event stream, the core must return the same ordered actions.

This enables:

- replay testing
- bug reproduction
- server/native consistency checks
- later migration to richer async without changing logic

## 7. Memory contract

- Native caller owns input bytes.
- Zig core owns output bytes until `core_free`.
- Output must be valid UTF-8 JSON in v0.1.
- `core_step_json` returns `0` on success and nonzero for FFI-level failure.
- Logical errors should be returned inside JSON with `ok: false`.

## 8. Internal Zig module layout

```text
zig-core/src/
  lib.zig       exports public API
  ffi.zig       C ABI, memory buffers, JSON bytes
  core.zig      Core struct and step function
  event.zig     event parsing/validation
  action.zig    action serialization
  codec.zig     JSON codec v0.1, future MessagePack/CBOR
  replay.zig    event replay utilities
```

## 9. Minimal core behavior for v0.1

The first implementation can support a small generic behavior:

- Accept any event with `type` string.
- For known demo event types, return useful demo actions.
- For unknown event types, return a `Log` action and `ok: true` or a typed error depending on strict mode.

Demo events:

- `CreateTask`
- `UpdateTask`
- `TransformText`
- `ImportFile`
- `NetworkSnapshotReceived`
- `ReplayEvents`

## 10. Zig tests

Required tests:

- FFI create/destroy.
- Valid event returns valid JSON.
- Invalid JSON returns structured error.
- Deterministic replay returns identical actions.
- Large payload within limit works.
- Payload over limit fails safely.
- Memory output can be freed repeatedly without leaks.
