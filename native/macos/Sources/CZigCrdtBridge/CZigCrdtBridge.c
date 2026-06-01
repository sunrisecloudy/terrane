#include "CZigCrdtBridge.h"

#include <dlfcn.h>
#include <stdlib.h>

typedef struct ZigCrdt ZigCrdt;

typedef struct ZigCrdtBuffer {
    uint8_t *ptr;
    size_t len;
} ZigCrdtBuffer;

typedef ZigCrdt *(*CrdtCreateFn)(void);
typedef void (*CrdtDestroyFn)(ZigCrdt *crdt);
typedef int32_t (*CrdtJsonFn)(
    ZigCrdt *crdt,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCrdtBuffer *output
);
typedef void (*CrdtFreeFn)(ZigCrdtBuffer buffer);

struct TerraneZigCrdt {
    void *handle;
    ZigCrdt *crdt;
    CrdtDestroyFn destroy;
    CrdtJsonFn apply_json;
    CrdtJsonFn merge_json;
    CrdtJsonFn materialize_json;
    CrdtFreeFn free_buffer;
};

static const char *last_error = NULL;

const char *terrane_zig_crdt_last_error(void) {
    return last_error;
}

TerraneZigCrdt *terrane_zig_crdt_open(const char *path) {
    last_error = NULL;
    void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (handle == NULL) {
        last_error = dlerror();
        return NULL;
    }

    CrdtCreateFn create = (CrdtCreateFn)dlsym(handle, "crdt_create");
    CrdtDestroyFn destroy = (CrdtDestroyFn)dlsym(handle, "crdt_destroy");
    CrdtJsonFn apply_json = (CrdtJsonFn)dlsym(handle, "crdt_apply_json");
    CrdtJsonFn merge_json = (CrdtJsonFn)dlsym(handle, "crdt_merge_json");
    CrdtJsonFn materialize_json = (CrdtJsonFn)dlsym(handle, "crdt_materialize_json");
    CrdtFreeFn free_buffer = (CrdtFreeFn)dlsym(handle, "crdt_free");
    if (create == NULL || destroy == NULL || apply_json == NULL || merge_json == NULL || materialize_json == NULL || free_buffer == NULL) {
        last_error = "libzig_crdt is missing required crdt_* symbols";
        dlclose(handle);
        return NULL;
    }

    ZigCrdt *crdt = create();
    if (crdt == NULL) {
        last_error = "crdt_create returned null";
        dlclose(handle);
        return NULL;
    }

    TerraneZigCrdt *bridge = (TerraneZigCrdt *)calloc(1, sizeof(TerraneZigCrdt));
    if (bridge == NULL) {
        destroy(crdt);
        dlclose(handle);
        last_error = "failed to allocate TerraneZigCrdt";
        return NULL;
    }

    bridge->handle = handle;
    bridge->crdt = crdt;
    bridge->destroy = destroy;
    bridge->apply_json = apply_json;
    bridge->merge_json = merge_json;
    bridge->materialize_json = materialize_json;
    bridge->free_buffer = free_buffer;
    return bridge;
}

void terrane_zig_crdt_close(TerraneZigCrdt *bridge) {
    if (bridge == NULL) {
        return;
    }
    if (bridge->destroy != NULL) {
        bridge->destroy(bridge->crdt);
    }
    if (bridge->handle != NULL) {
        dlclose(bridge->handle);
    }
    free(bridge);
}

static int32_t call_crdt_json(
    TerraneZigCrdt *bridge,
    CrdtJsonFn function,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
) {
    if (output_ptr == NULL || output_len == NULL) {
        return -10;
    }
    *output_ptr = NULL;
    *output_len = 0;

    if (bridge == NULL || function == NULL) {
        return -11;
    }

    ZigCrdtBuffer output = {0};
    const int32_t code = function(bridge->crdt, input_ptr, input_len, &output);
    if (code != 0) {
        return code;
    }

    *output_ptr = output.ptr;
    *output_len = output.len;
    return 0;
}

int32_t terrane_zig_crdt_apply_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
) {
    return call_crdt_json(bridge, bridge != NULL ? bridge->apply_json : NULL, input_ptr, input_len, output_ptr, output_len);
}

int32_t terrane_zig_crdt_merge_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
) {
    return call_crdt_json(bridge, bridge != NULL ? bridge->merge_json : NULL, input_ptr, input_len, output_ptr, output_len);
}

int32_t terrane_zig_crdt_materialize_json(
    TerraneZigCrdt *bridge,
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **output_ptr,
    size_t *output_len
) {
    return call_crdt_json(bridge, bridge != NULL ? bridge->materialize_json : NULL, input_ptr, input_len, output_ptr, output_len);
}

void terrane_zig_crdt_free_output(
    TerraneZigCrdt *bridge,
    uint8_t *output_ptr,
    size_t output_len
) {
    if (bridge == NULL || bridge->free_buffer == NULL || output_ptr == NULL) {
        return;
    }

    ZigCrdtBuffer buffer = {
        .ptr = output_ptr,
        .len = output_len,
    };
    bridge->free_buffer(buffer);
}
