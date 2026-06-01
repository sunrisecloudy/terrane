#ifndef C_ZIG_CORE_BRIDGE_H
#define C_ZIG_CORE_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TerraneZigCore TerraneZigCore;

TerraneZigCore *terrane_zig_core_open(const char *path);
void terrane_zig_core_close(TerraneZigCore *bridge);

int32_t terrane_zig_core_step_json(
    TerraneZigCore *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

void terrane_zig_core_free_output(
    TerraneZigCore *bridge,
    uint8_t *output_ptr,
    size_t output_len
);

const char *terrane_zig_core_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
