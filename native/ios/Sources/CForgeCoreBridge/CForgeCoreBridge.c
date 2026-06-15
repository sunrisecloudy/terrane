#include "CForgeCoreBridge.h"

#include <dlfcn.h>
#include <stdbool.h>
#include <stdlib.h>

typedef void *(*ForgeCoreOpenInMemoryFn)(const char *workspace_id);
typedef char *(*ForgeCoreHandleCommandFn)(void *core, const char *command_json);
typedef char *(*ForgeCoreDrainEventsFn)(void *core);
typedef char *(*ForgeCoreLastErrorFn)(void);
typedef void (*ForgeCoreCloseFn)(void *core);
typedef void (*ForgeStringFreeFn)(char *value);

struct TerraneForgeCore {
    void *library;
    void *core;
    ForgeCoreHandleCommandFn handle_command;
    ForgeCoreDrainEventsFn drain_events;
    ForgeCoreLastErrorFn last_error;
    ForgeCoreCloseFn close_core;
    ForgeStringFreeFn free_string;
    bool linked;
};

static const char *last_error = NULL;

static TerraneForgeCore *open_with_symbols(
    void *library,
    ForgeCoreOpenInMemoryFn open_in_memory,
    ForgeCoreHandleCommandFn handle_command,
    ForgeCoreDrainEventsFn drain_events,
    ForgeCoreLastErrorFn ffi_last_error,
    ForgeCoreCloseFn close_core,
    ForgeStringFreeFn free_string,
    bool linked,
    const char *workspace_id
);

char *terrane_forge_core_last_error(void) {
    return last_error == NULL ? NULL : (char *)last_error;
}

TerraneForgeCore *terrane_forge_core_open_in_memory(const char *library_path, const char *workspace_id) {
    last_error = NULL;
    if (workspace_id == NULL) {
        last_error = "workspace_id is required";
        return NULL;
    }

    if (library_path == NULL) {
        return open_with_symbols(
            NULL,
            (ForgeCoreOpenInMemoryFn)dlsym(RTLD_DEFAULT, "forge_core_open_in_memory"),
            (ForgeCoreHandleCommandFn)dlsym(RTLD_DEFAULT, "forge_core_handle_command"),
            (ForgeCoreDrainEventsFn)dlsym(RTLD_DEFAULT, "forge_core_drain_events"),
            (ForgeCoreLastErrorFn)dlsym(RTLD_DEFAULT, "forge_core_last_error"),
            (ForgeCoreCloseFn)dlsym(RTLD_DEFAULT, "forge_core_close"),
            (ForgeStringFreeFn)dlsym(RTLD_DEFAULT, "forge_string_free"),
            true,
            workspace_id
        );
    }

    void *library = dlopen(library_path, RTLD_NOW | RTLD_LOCAL);
    if (library == NULL) {
        last_error = dlerror();
        return NULL;
    }

    TerraneForgeCore *bridge = open_with_symbols(
        library,
        (ForgeCoreOpenInMemoryFn)dlsym(library, "forge_core_open_in_memory"),
        (ForgeCoreHandleCommandFn)dlsym(library, "forge_core_handle_command"),
        (ForgeCoreDrainEventsFn)dlsym(library, "forge_core_drain_events"),
        (ForgeCoreLastErrorFn)dlsym(library, "forge_core_last_error"),
        (ForgeCoreCloseFn)dlsym(library, "forge_core_close"),
        (ForgeStringFreeFn)dlsym(library, "forge_string_free"),
        false,
        workspace_id
    );
    if (bridge == NULL) {
        dlclose(library);
    }
    return bridge;
}

static TerraneForgeCore *open_with_symbols(
    void *library,
    ForgeCoreOpenInMemoryFn open_in_memory,
    ForgeCoreHandleCommandFn handle_command,
    ForgeCoreDrainEventsFn drain_events,
    ForgeCoreLastErrorFn ffi_last_error,
    ForgeCoreCloseFn close_core,
    ForgeStringFreeFn free_string,
    bool linked,
    const char *workspace_id
) {
    if (
        open_in_memory == NULL ||
        handle_command == NULL ||
        drain_events == NULL ||
        ffi_last_error == NULL ||
        close_core == NULL ||
        free_string == NULL
    ) {
        last_error = "libforge_ffi is missing required forge_core_* symbols";
        return NULL;
    }

    void *core = open_in_memory(workspace_id);
    if (core == NULL) {
        char *error = ffi_last_error();
        if (error != NULL) {
            last_error = "forge_core_open_in_memory returned null; see forge_core_last_error";
            free_string(error);
        } else {
            last_error = "forge_core_open_in_memory returned null";
        }
        return NULL;
    }

    TerraneForgeCore *bridge = (TerraneForgeCore *)calloc(1, sizeof(TerraneForgeCore));
    if (bridge == NULL) {
        close_core(core);
        last_error = "failed to allocate TerraneForgeCore";
        return NULL;
    }

    bridge->library = library;
    bridge->core = core;
    bridge->handle_command = handle_command;
    bridge->drain_events = drain_events;
    bridge->last_error = ffi_last_error;
    bridge->close_core = close_core;
    bridge->free_string = free_string;
    bridge->linked = linked;
    return bridge;
}

void terrane_forge_core_close(TerraneForgeCore *bridge) {
    if (bridge == NULL) {
        return;
    }
    if (bridge->close_core != NULL && bridge->core != NULL) {
        bridge->close_core(bridge->core);
    }
    if (!bridge->linked && bridge->library != NULL) {
        dlclose(bridge->library);
    }
    free(bridge);
}

char *terrane_forge_core_handle_command(TerraneForgeCore *bridge, const char *command_json) {
    if (bridge == NULL || bridge->handle_command == NULL || command_json == NULL) {
        last_error = "bridge and command_json are required";
        return NULL;
    }
    last_error = NULL;
    return bridge->handle_command(bridge->core, command_json);
}

char *terrane_forge_core_drain_events(TerraneForgeCore *bridge) {
    if (bridge == NULL || bridge->drain_events == NULL) {
        last_error = "bridge is required";
        return NULL;
    }
    last_error = NULL;
    return bridge->drain_events(bridge->core);
}

void terrane_forge_core_free_string(TerraneForgeCore *bridge, char *value) {
    if (value == NULL) {
        return;
    }
    if (bridge == NULL || bridge->free_string == NULL) {
        return;
    }
    bridge->free_string(value);
}
