#pragma once

#include "bridge_types.h"
#include "platform_dialogs.h"
#include "platform_network.h"
#include "platform_notifications.h"
#include "platform_storage.h"
#include "zig_core_bridge.h"

typedef struct {
  PlatformStorage *storage;
  PlatformDialogs dialogs;
  PlatformNotifications notifications;
  PlatformNetwork network;
  ZigCoreBridge core;
} WebBridge;

WebBridge *web_bridge_new(const gchar *database_path);
void web_bridge_free(WebBridge *bridge);
gchar *web_bridge_handle_json(WebBridge *bridge, const gchar *body, AppSandboxContext context);
