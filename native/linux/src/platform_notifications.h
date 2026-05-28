#pragma once

#include "bridge_types.h"

typedef struct {
  int reserved;
} PlatformNotifications;

JsonNode *platform_notifications_toast(PlatformNotifications *notifications, const BridgeRequest *request);
