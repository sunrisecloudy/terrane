#pragma once

#include "bridge_types.h"

#include <sqlite3.h>

typedef struct {
  sqlite3 *db;
} PlatformStorage;

PlatformStorage *platform_storage_new(const gchar *database_path);
void platform_storage_free(PlatformStorage *storage);
JsonNode *platform_storage_get(PlatformStorage *storage, const BridgeRequest *request);
JsonNode *platform_storage_set(PlatformStorage *storage, const BridgeRequest *request);
JsonNode *platform_storage_remove(PlatformStorage *storage, const BridgeRequest *request);
JsonNode *platform_storage_list(PlatformStorage *storage, const BridgeRequest *request);
