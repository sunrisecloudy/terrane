#pragma once

#include "bridge_types.h"

#include <gtk/gtk.h>

typedef struct {
  GtkWindow *owner;
} PlatformDialogs;

void platform_dialogs_init(PlatformDialogs *dialogs, GtkWindow *owner);
JsonNode *platform_dialogs_open_file(PlatformDialogs *dialogs, const BridgeRequest *request);
JsonNode *platform_dialogs_save_file(PlatformDialogs *dialogs, const BridgeRequest *request);
