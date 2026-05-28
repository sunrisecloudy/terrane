#pragma once

#include "bridge_types.h"

typedef struct {
  int reserved;
} PlatformNetwork;

JsonNode *platform_network_request(PlatformNetwork *network, const BridgeRequest *request);
