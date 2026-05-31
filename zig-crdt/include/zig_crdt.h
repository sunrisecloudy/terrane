#ifndef ZIG_CRDT_H
#define ZIG_CRDT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ZigCrdt ZigCrdt;

typedef struct ZigCrdtBuffer {
    uint8_t *ptr;
    size_t len;
} ZigCrdtBuffer;

ZigCrdt *crdt_create(void);
void crdt_destroy(ZigCrdt *crdt);

int32_t crdt_apply_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

int32_t crdt_merge_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

int32_t crdt_materialize_json(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);

void crdt_free(ZigCrdtBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
