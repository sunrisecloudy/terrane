# Zig Core Target

Codex should implement this as the shared deterministic core library.

Required C ABI:

```c
ZigCore *core_create(void);
void core_destroy(ZigCore *core);
int32_t core_step_json(ZigCore *core, const uint8_t *input_ptr, size_t input_len, ZigCoreBuffer *output);
void core_free(ZigCoreBuffer buffer);
```

MVP acceptance:

- Builds as native static/shared library.
- Builds target-specific artifacts for mobile/desktop.
- `zig build test` passes.
- FFI tests pass.
- Invalid input never crashes.
