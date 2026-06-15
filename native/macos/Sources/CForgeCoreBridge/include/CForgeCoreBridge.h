#ifndef C_FORGE_CORE_BRIDGE_H
#define C_FORGE_CORE_BRIDGE_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TerraneForgeCore TerraneForgeCore;

TerraneForgeCore *terrane_forge_core_open(const char *library_path, const char *database_path, const char *workspace_id);
void terrane_forge_core_close(TerraneForgeCore *bridge);

char *terrane_forge_core_handle_command(TerraneForgeCore *bridge, const char *command_json);
char *terrane_forge_core_drain_events(TerraneForgeCore *bridge);
char *terrane_forge_core_last_error(void);
void terrane_forge_core_free_string(TerraneForgeCore *bridge, char *value);

#ifdef __cplusplus
}
#endif

#endif
