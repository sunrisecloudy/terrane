#pragma once

#include "bridge_types.h"

#include <sqlite3.h>

typedef struct {
  sqlite3 *db;
} PlatformNetwork;

JsonNode *platform_network_request(PlatformNetwork *network, const BridgeRequest *request);
