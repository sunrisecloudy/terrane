# Zig Core

Shared deterministic core library for generated app event handling. The public ABI is
declared in `include/zig_core.h` and implemented by `src/lib.zig`.

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

Current local verification command:

```sh
zig test src/lib.zig -target aarch64-macos.15.0 -lc
```

On this macOS 26 host, `zig build test` currently fails before evaluating
`build.zig` because Zig's build runner links against an invalid native
`macos.26.4.1` target. The direct test command above pins a stable macOS target
and exercises the same unit tests.
