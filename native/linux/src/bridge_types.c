#include "bridge_types.h"

JsonNode *bridge_success(const BridgeRequest *request, JsonNode *result) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  if (request != NULL && request->has_id) {
    json_builder_set_member_name(builder, "id");
    json_builder_add_string_value(builder, request->id);
  }
  json_builder_set_member_name(builder, "result");
  json_builder_add_value(builder, result);
  json_builder_end_object(builder);
  return json_builder_get_root(builder);
}

JsonNode *bridge_failure(const BridgeRequest *request, const gchar *code, const gchar *message, JsonObject *details) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, FALSE);
  if (request != NULL && request->has_id) {
    json_builder_set_member_name(builder, "id");
    json_builder_add_string_value(builder, request->id);
  }
  json_builder_set_member_name(builder, "error");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "code");
  json_builder_add_string_value(builder, code);
  json_builder_set_member_name(builder, "message");
  json_builder_add_string_value(builder, message);
  json_builder_set_member_name(builder, "details");
  json_builder_add_value(builder, json_node_init_object(json_node_alloc(), details != NULL ? details : json_object_new()));
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  return json_builder_get_root(builder);
}

gchar *bridge_response_to_string(JsonNode *response) {
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, response);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  return text;
}

void network_policy_rule_free(gpointer data) {
  NetworkPolicyRule *rule = data;
  if (rule == NULL) {
    return;
  }
  g_clear_pointer(&rule->origin, g_free);
  g_clear_pointer(&rule->path_prefix, g_free);
  if (rule->methods != NULL) {
    g_hash_table_unref(rule->methods);
  }
  if (rule->allowed_headers != NULL) {
    g_hash_table_unref(rule->allowed_headers);
  }
  g_free(rule);
}

void bridge_request_clear(BridgeRequest *request) {
  if (request == NULL) {
    return;
  }
  g_clear_pointer(&request->id, g_free);
  g_clear_pointer(&request->method, g_free);
  if (request->params != NULL) {
    json_object_unref(request->params);
    request->params = NULL;
  }
  app_sandbox_context_clear(&request->context);
}

void app_sandbox_context_clear(AppSandboxContext *context) {
  if (context == NULL) {
    return;
  }
  g_clear_pointer(&context->app_id, g_free);
  g_clear_pointer(&context->storage_prefix, g_free);
  g_clear_pointer(&context->mount_token, g_free);
  if (context->approved_permissions != NULL) {
    g_hash_table_unref(context->approved_permissions);
    context->approved_permissions = NULL;
  }
  if (context->network_policy != NULL) {
    g_ptr_array_unref(context->network_policy);
    context->network_policy = NULL;
  }
  if (context->resource_budget != NULL) {
    g_hash_table_unref(context->resource_budget);
    context->resource_budget = NULL;
  }
}
