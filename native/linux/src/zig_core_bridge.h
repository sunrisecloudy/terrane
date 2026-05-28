#pragma once

#include "bridge_types.h"

typedef struct {
  gpointer handle;
  gpointer core;
  gpointer destroy;
  gpointer step_json;
  gpointer free_buffer;
  gchar *loaded_path;
} ZigCoreBridge;

void zig_core_bridge_init(ZigCoreBridge *core);
void zig_core_bridge_clear(ZigCoreBridge *core);
gboolean zig_core_bridge_is_available(const ZigCoreBridge *core);
JsonNode *zig_core_bridge_step(ZigCoreBridge *core, const BridgeRequest *request);
