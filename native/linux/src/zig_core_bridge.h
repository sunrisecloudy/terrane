#pragma once

#include "bridge_types.h"

typedef struct {
  int reserved;
} ZigCoreBridge;

JsonNode *zig_core_bridge_step(ZigCoreBridge *core, const BridgeRequest *request);
