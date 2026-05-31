#ifndef C_ZIG_CRDT_BRIDGE_H
#define C_ZIG_CRDT_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct NativeAIZigCrdt NativeAIZigCrdt;

NativeAIZigCrdt *native_ai_zig_crdt_open(const char *path);
void native_ai_zig_crdt_close(NativeAIZigCrdt *bridge);

int32_t native_ai_zig_crdt_apply_json(
    NativeAIZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

int32_t native_ai_zig_crdt_merge_json(
    NativeAIZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

int32_t native_ai_zig_crdt_materialize_json(
    NativeAIZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

void native_ai_zig_crdt_free_output(
    NativeAIZigCrdt *bridge,
    uint8_t *output_ptr,
    size_t output_len
);

const char *native_ai_zig_crdt_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
