#pragma once

#include "bridge_types.h"

typedef struct {
  int reserved;
} PlatformDialogs;

JsonNode *platform_dialogs_open_file(PlatformDialogs *dialogs, const BridgeRequest *request);
JsonNode *platform_dialogs_save_file(PlatformDialogs *dialogs, const BridgeRequest *request);
