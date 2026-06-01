#ifndef C_ZIG_CRDT_BRIDGE_H
#define C_ZIG_CRDT_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TerraneZigCrdt TerraneZigCrdt;

TerraneZigCrdt *terrane_zig_crdt_open(const char *path);
void terrane_zig_crdt_close(TerraneZigCrdt *bridge);

int32_t terrane_zig_crdt_apply_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

int32_t terrane_zig_crdt_merge_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

int32_t terrane_zig_crdt_materialize_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
);

void terrane_zig_crdt_free_output(
    TerraneZigCrdt *bridge,
    uint8_t *output_ptr,
    size_t output_len
);

const char *terrane_zig_crdt_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
