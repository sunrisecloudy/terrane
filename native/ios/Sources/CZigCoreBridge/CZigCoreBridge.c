#include "CZigCoreBridge.h"

#include <dlfcn.h>
#include <stdbool.h>
#include <stdlib.h>

typedef struct ZigCore ZigCore;

typedef struct ZigCoreBuffer {
    uint8_t *ptr;
    size_t len;
} ZigCoreBuffer;

typedef ZigCore *(*CoreCreateFn)(void);
typedef void (*CoreDestroyFn)(ZigCore *core);
typedef int32_t (*CoreStepJsonFn)(
    ZigCore *core,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCoreBuffer *output
);
typedef void (*CoreFreeFn)(ZigCoreBuffer buffer);

extern ZigCore *core_create(void) __attribute__((weak_import));
extern void core_destroy(ZigCore *core) __attribute__((weak_import));
extern int32_t core_step_json(
    ZigCore *core,
    const uint8_t *input_ptr,
    size_t input_len,
    ZigCoreBuffer *output
) __attribute__((weak_import));
extern void core_free(ZigCoreBuffer buffer) __attribute__((weak_import));

struct NativeAIZigCore {
    void *handle;
    ZigCore *core;
    CoreDestroyFn destroy;
    CoreStepJsonFn step_json;
    CoreFreeFn free_buffer;
    bool linked;
};

static NativeAIZigCore *open_with_symbols(
    void *handle,
    CoreCreateFn create,
    CoreDestroyFn destroy,
    CoreStepJsonFn step_json,
    CoreFreeFn free_buffer,
    bool linked
) {
    if (create == NULL || destroy == NULL || step_json == NULL || free_buffer == NULL) {
        return NULL;
    }

    ZigCore *core = create();
    if (core == NULL) {
        return NULL;
    }

    NativeAIZigCore *bridge = (NativeAIZigCore *)calloc(1, sizeof(NativeAIZigCore));
    if (bridge == NULL) {
        destroy(core);
        return NULL;
    }

    bridge->handle = handle;
    bridge->core = core;
    bridge->destroy = destroy;
    bridge->step_json = step_json;
    bridge->free_buffer = free_buffer;
    bridge->linked = linked;
    return bridge;
}

NativeAIZigCore *native_ai_zig_core_open(const char *path) {
    if (path == NULL) {
        return open_with_symbols(
            NULL,
            core_create,
            core_destroy,
            core_step_json,
            core_free,
            true
        );
    }

    void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (handle == NULL) {
        return NULL;
    }

    NativeAIZigCore *bridge = open_with_symbols(
        handle,
        (CoreCreateFn)dlsym(handle, "core_create"),
        (CoreDestroyFn)dlsym(handle, "core_destroy"),
        (CoreStepJsonFn)dlsym(handle, "core_step_json"),
        (CoreFreeFn)dlsym(handle, "core_free"),
        false
    );
    if (bridge == NULL) {
        dlclose(handle);
    }
    return bridge;
}

void native_ai_zig_core_close(NativeAIZigCore *bridge) {
    if (bridge == NULL) {
        return;
    }
    if (bridge->destroy != NULL) {
        bridge->destroy(bridge->core);
    }
    if (!bridge->linked && bridge->handle != NULL) {
        dlclose(bridge->handle);
    }
    free(bridge);
}

int32_t native_ai_zig_core_step_json(
    NativeAIZigCore *bridge,
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

    if (bridge == NULL || bridge->step_json == NULL) {
        return -11;
    }

    ZigCoreBuffer output = {0};
    const int32_t code = bridge->step_json(bridge->core, input_ptr, input_len, &output);
    if (code != 0) {
        return code;
    }

    *output_ptr = output.ptr;
    *output_len = output.len;
    return 0;
}

void native_ai_zig_core_free_output(
    NativeAIZigCore *bridge,
    uint8_t *output_ptr,
    size_t output_len
) {
    if (bridge == NULL || bridge->free_buffer == NULL || output_ptr == NULL) {
        return;
    }

    ZigCoreBuffer buffer = {
        .ptr = output_ptr,
        .len = output_len,
    };
    bridge->free_buffer(buffer);
}
