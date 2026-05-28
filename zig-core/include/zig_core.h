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
