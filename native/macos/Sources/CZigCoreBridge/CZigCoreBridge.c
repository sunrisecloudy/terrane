#include "CZigCoreBridge.h"

#include <dlfcn.h>
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

struct NativeAIZigCore {
    void *handle;
    ZigCore *core;
    CoreDestroyFn destroy;
    CoreStepJsonFn step_json;
    CoreFreeFn free_buffer;
};

static const char *last_error = NULL;

const char *native_ai_zig_core_last_error(void) {
    return last_error;
}

NativeAIZigCore *native_ai_zig_core_open(const char *path) {
    last_error = NULL;
    void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (handle == NULL) {
        last_error = dlerror();
        return NULL;
    }

    CoreCreateFn create = (CoreCreateFn)dlsym(handle, "core_create");
    CoreDestroyFn destroy = (CoreDestroyFn)dlsym(handle, "core_destroy");
    CoreStepJsonFn step_json = (CoreStepJsonFn)dlsym(handle, "core_step_json");
    CoreFreeFn free_buffer = (CoreFreeFn)dlsym(handle, "core_free");
    if (create == NULL || destroy == NULL || step_json == NULL || free_buffer == NULL) {
        last_error = "libzig_core is missing required core_* symbols";
        dlclose(handle);
        return NULL;
    }

    ZigCore *core = create();
    if (core == NULL) {
        last_error = "core_create returned null";
        dlclose(handle);
        return NULL;
    }

    NativeAIZigCore *bridge = (NativeAIZigCore *)calloc(1, sizeof(NativeAIZigCore));
    if (bridge == NULL) {
        destroy(core);
        dlclose(handle);
        last_error = "failed to allocate NativeAIZigCore";
        return NULL;
    }

    bridge->handle = handle;
    bridge->core = core;
    bridge->destroy = destroy;
    bridge->step_json = step_json;
    bridge->free_buffer = free_buffer;
    return bridge;
}

void native_ai_zig_core_close(NativeAIZigCore *bridge) {
    if (bridge == NULL) {
        return;
    }
    if (bridge->destroy != NULL) {
        bridge->destroy(bridge->core);
    }
    if (bridge->handle != NULL) {
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
