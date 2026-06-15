#pragma once

#include "bridge_types.h"
#include "platform_dialogs.h"
#include "platform_network.h"
#include "platform_notifications.h"
#include "platform_storage.h"
#include "forge_core_bridge.h"

typedef struct {
  PlatformStorage *storage;
  PlatformDialogs dialogs;
  PlatformNotifications notifications;
  PlatformNetwork network;
  ForgeCoreBridge core;
} WebBridge;

WebBridge *web_bridge_new(const gchar *database_path, GtkWindow *owner_window);
void web_bridge_free(WebBridge *bridge);
gchar *web_bridge_handle_json(WebBridge *bridge, const gchar *body, AppSandboxContext context);
