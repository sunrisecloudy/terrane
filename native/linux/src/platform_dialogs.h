#pragma once

#include "bridge_types.h"

#include <gtk/gtk.h>
#include <sqlite3.h>

typedef struct {
  GtkWindow *owner;
  sqlite3 *db;
} PlatformDialogs;

void platform_dialogs_init(PlatformDialogs *dialogs, GtkWindow *owner, sqlite3 *db);
JsonNode *platform_dialogs_open_file(PlatformDialogs *dialogs, const BridgeRequest *request);
JsonNode *platform_dialogs_save_file(PlatformDialogs *dialogs, const BridgeRequest *request);
