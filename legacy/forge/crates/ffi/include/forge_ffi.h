#ifndef FORGE_FFI_H
#define FORGE_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ForgeCoreHandle ForgeCoreHandle;

ForgeCoreHandle *forge_core_open(const char *path, const char *workspace_id);
ForgeCoreHandle *forge_core_open_in_memory(const char *workspace_id);

char *forge_core_handle_command(ForgeCoreHandle *handle, const char *command_json);
char *forge_core_drain_events(ForgeCoreHandle *handle);
char *forge_core_last_error(void);

void forge_core_close(ForgeCoreHandle *handle);
void forge_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
