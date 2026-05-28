#include "web_bridge.h"

static const gchar *permission_for_bridge_method(const gchar *method) {
  if (g_strcmp0(method, "storage.get") == 0 || g_strcmp0(method, "storage.list") == 0) {
    return "storage.read";
  }
  if (g_strcmp0(method, "storage.set") == 0 || g_strcmp0(method, "storage.remove") == 0) {
    return "storage.write";
  }
  if (g_strcmp0(method, "dialog.openFile") == 0 || g_strcmp0(method, "dialog.saveFile") == 0 ||
      g_strcmp0(method, "notification.toast") == 0 || g_strcmp0(method, "network.request") == 0 ||
      g_strcmp0(method, "core.step") == 0) {
    return method;
  }
  return NULL;
}

static gboolean approved_permissions_contains(AppSandboxContext *context, const gchar *permission) {
  return context->approved_permissions != NULL && g_hash_table_contains(context->approved_permissions, permission);
}

static JsonNode *capabilities_response(const BridgeRequest *request) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "platform");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "runtimeVersion");
  json_builder_add_string_value(builder, "0.1.0");
  json_builder_set_member_name(builder, "devMode");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "features");
  json_builder_begin_object(builder);
  const gchar *enabled[] = {"storage.get", "storage.set", "storage.remove", "storage.list", "notification.toast", "runtime.capabilities", "app.log"};
  for (gsize index = 0; index < G_N_ELEMENTS(enabled); ++index) {
    json_builder_set_member_name(builder, enabled[index]);
    json_builder_add_boolean_value(builder, TRUE);
  }
  const gchar *disabled[] = {"dialog.openFile", "dialog.saveFile", "network.request", "core.step"};
  for (gsize index = 0; index < G_N_ELEMENTS(disabled); ++index) {
    json_builder_set_member_name(builder, disabled[index]);
    json_builder_add_boolean_value(builder, FALSE);
  }
  json_builder_end_object(builder);
  json_builder_set_member_name(builder, "limits");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "maxPackageBytes");
  json_builder_add_int_value(builder, 1048576);
  json_builder_set_member_name(builder, "maxFileBytes");
  json_builder_add_int_value(builder, 524288);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}

static JsonNode *dispatch(WebBridge *bridge, const BridgeRequest *request) {
  if (g_strcmp0(request->method, "storage.get") == 0) {
    return platform_storage_get(bridge->storage, request);
  }
  if (g_strcmp0(request->method, "storage.set") == 0) {
    return platform_storage_set(bridge->storage, request);
  }
  if (g_strcmp0(request->method, "storage.remove") == 0) {
    return platform_storage_remove(bridge->storage, request);
  }
  if (g_strcmp0(request->method, "storage.list") == 0) {
    return platform_storage_list(bridge->storage, request);
  }
  if (g_strcmp0(request->method, "dialog.openFile") == 0) {
    return platform_dialogs_open_file(&bridge->dialogs, request);
  }
  if (g_strcmp0(request->method, "dialog.saveFile") == 0) {
    return platform_dialogs_save_file(&bridge->dialogs, request);
  }
  if (g_strcmp0(request->method, "notification.toast") == 0) {
    return platform_notifications_toast(&bridge->notifications, request);
  }
  if (g_strcmp0(request->method, "network.request") == 0) {
    return platform_network_request(&bridge->network, request);
  }
  if (g_strcmp0(request->method, "core.step") == 0) {
    return zig_core_bridge_step(&bridge->core, request);
  }
  if (g_strcmp0(request->method, "runtime.capabilities") == 0) {
    return capabilities_response(request);
  }
  if (g_strcmp0(request->method, "app.log") == 0) {
    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "ok");
    json_builder_add_boolean_value(builder, TRUE);
    json_builder_end_object(builder);
    return bridge_success(request, json_builder_get_root(builder));
  }
  return bridge_failure(request, "unknown_method", "Unknown bridge method", NULL);
}

WebBridge *web_bridge_new(const gchar *database_path) {
  WebBridge *bridge = g_new0(WebBridge, 1);
  bridge->storage = platform_storage_new(database_path);
  return bridge;
}

void web_bridge_free(WebBridge *bridge) {
  if (bridge == NULL) {
    return;
  }
  platform_storage_free(bridge->storage);
  g_free(bridge);
}

gchar *web_bridge_handle_json(WebBridge *bridge, const gchar *body, AppSandboxContext context) {
  BridgeRequest request = {.context = context};
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, body, -1, NULL)) {
    JsonNode *error = bridge_failure(NULL, "invalid_request", "Bridge message body must be JSON", NULL);
    return bridge_response_to_string(error);
  }

  JsonObject *root = json_node_get_object(json_parser_get_root(parser));
  if (json_object_has_member(root, "id")) {
    request.has_id = TRUE;
    request.id = g_strdup(json_object_get_string_member(root, "id"));
  }
  request.method = g_strdup(json_object_get_string_member_with_default(root, "method", ""));
  request.params = json_object_ref(json_object_get_object_member(root, "params"));

  const gchar *permission = permission_for_bridge_method(request.method);
  if (permission != NULL && !approved_permissions_contains(&request.context, permission)) {
    JsonObject *details = json_object_new();
    json_object_set_string_member(details, "appId", request.context.app_id);
    json_object_set_string_member(details, "method", request.method);
    json_object_set_string_member(details, "requiredPermission", permission);
    JsonNode *response = bridge_failure(&request, "permission_denied", "App cannot call requested bridge method", details);
    return bridge_response_to_string(response);
  }

  JsonNode *response = dispatch(bridge, &request);
  gchar *text = bridge_response_to_string(response);
  bridge_request_clear(&request);
  g_object_unref(parser);
  return text;
}
