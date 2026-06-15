#pragma once

#include "bridge_types.h"

typedef struct {
  gpointer handle;
  gpointer core;
  gpointer close_core;
  gpointer handle_command;
  gpointer drain_events;
  gpointer last_error;
  gpointer free_string;
  gchar *loaded_path;
} ForgeCoreBridge;

void forge_core_bridge_init(ForgeCoreBridge *core);
void forge_core_bridge_clear(ForgeCoreBridge *core);
gboolean forge_core_bridge_is_available(const ForgeCoreBridge *core);
JsonNode *forge_core_bridge_step(ForgeCoreBridge *core, const BridgeRequest *request);
