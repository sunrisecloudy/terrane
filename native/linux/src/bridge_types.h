#pragma once

#include <glib.h>
#include <json-glib/json-glib.h>

typedef struct {
  gchar *app_id;
  gchar *storage_prefix;
  GHashTable *approved_permissions;
} AppSandboxContext;

typedef struct {
  gchar *id;
  gboolean has_id;
  gchar *method;
  JsonObject *params;
  AppSandboxContext context;
} BridgeRequest;

JsonNode *bridge_success(const BridgeRequest *request, JsonNode *result);
JsonNode *bridge_failure(const BridgeRequest *request, const gchar *code, const gchar *message, JsonObject *details);
gchar *bridge_response_to_string(JsonNode *response);
void bridge_request_clear(BridgeRequest *request);
void app_sandbox_context_clear(AppSandboxContext *context);
