#ifndef C_ZIG_CORE_BRIDGE_H
#define C_ZIG_CORE_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct NativeAIZigCore NativeAIZigCore;

NativeAIZigCore *native_ai_zig_core_open(const char *path);
void native_ai_zig_core_close(NativeAIZigCore *bridge);

int32_t native_ai_zig_core_step_json(
    NativeAIZigCore *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

void native_ai_zig_core_free_output(
    NativeAIZigCore *bridge,
    uint8_t *output_ptr,
    size_t output_len
);

const char *native_ai_zig_core_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
