#pragma once

#include <glib.h>
#include <json-glib/json-glib.h>

typedef struct {
  gchar *origin;
  GHashTable *methods;
  GHashTable *allowed_headers;
  gsize max_request_bytes;
  gsize max_response_bytes;
  guint timeout_ms;
} NetworkPolicyRule;

typedef struct {
  gchar *app_id;
  gchar *storage_prefix;
  GHashTable *approved_permissions;
  GPtrArray *network_policy;
  gboolean deny_private_network;
  gchar *mount_token;
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
void network_policy_rule_free(gpointer data);
void bridge_request_clear(BridgeRequest *request);
void app_sandbox_context_clear(AppSandboxContext *context);
