#include "web_bridge.h"

static gchar *runtime_session_id(const BridgeRequest *request) {
  const gchar *app_id = request->context.app_id != NULL ? request->context.app_id : "";
  const gchar *mount_token = request->context.mount_token != NULL && request->context.mount_token[0] != '\0'
      ? request->context.mount_token
      : "native";
  return g_strdup_printf("runtime_linux_%s_%s", app_id, mount_token);
}

static gchar *new_bridge_call_id(void) {
  static gint sequence = 0;
  gint next = g_atomic_int_add(&sequence, 1);
  return g_strdup_printf("bridge_linux_%" G_GINT64_FORMAT "_%d", g_get_real_time(), next);
}

static gchar *new_core_id(const gchar *prefix) {
  static gint sequence = 0;
  gint next = g_atomic_int_add(&sequence, 1);
  return g_strdup_printf("%s_linux_%" G_GINT64_FORMAT "_%d", prefix, g_get_real_time(), next);
}

static void bind_text(sqlite3_stmt *statement, int index, const gchar *value) {
  sqlite3_bind_text(statement, index, value != NULL ? value : "", -1, SQLITE_TRANSIENT);
}

static void bind_nullable_text(sqlite3_stmt *statement, int index, const gchar *value) {
  if (value == NULL || value[0] == '\0') {
    sqlite3_bind_null(statement, index);
    return;
  }
  sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT);
}

static void bind_nullable_int64(sqlite3_stmt *statement, int index, gboolean has_value, gint64 value) {
  if (!has_value) {
    sqlite3_bind_null(statement, index);
    return;
  }
  sqlite3_bind_int64(statement, index, (sqlite3_int64)value);
}

static gchar *json_node_to_string_copy(JsonNode *node) {
  if (node == NULL) {
    return NULL;
  }
  JsonNode *copy = json_node_copy(node);
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, copy);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  json_node_unref(copy);
  return text;
}

static gchar *json_object_to_string(JsonObject *object) {
  if (object == NULL) {
    return g_strdup("{}");
  }
  JsonNode *node = json_node_init_object(json_node_alloc(), json_object_ref(object));
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, node);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  json_node_unref(node);
  return text;
}

static gchar *json_member_to_string(JsonObject *object, const gchar *member) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return NULL;
  }
  JsonNode *copy = json_node_copy(json_object_get_member(object, member));
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, copy);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  json_node_unref(copy);
  return text;
}

static gboolean state_version_before(JsonObject *result, gint64 *out) {
  if (result == NULL || !json_object_has_member(result, "stateVersion")) {
    return FALSE;
  }
  JsonNode *node = json_object_get_member(result, "stateVersion");
  if (!JSON_NODE_HOLDS_VALUE(node)) {
    return FALSE;
  }
  GType value_type = json_node_get_value_type(node);
  if (value_type != G_TYPE_INT64 && value_type != G_TYPE_INT && value_type != G_TYPE_DOUBLE) {
    return FALSE;
  }
  gint64 value = value_type == G_TYPE_DOUBLE ? (gint64)json_node_get_double(node) : json_node_get_int(node);
  *out = value > 0 ? value - 1 : 0;
  return TRUE;
}

static void ensure_runtime_session(WebBridge *bridge, const BridgeRequest *request) {
  if (bridge == NULL || bridge->storage == NULL || bridge->storage->db == NULL || request->context.app_id == NULL) {
    return;
  }
  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "INSERT INTO runtime_sessions "
      "(session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json) "
      "VALUES (?, 'linux', 'linux', '0.1.0', ?, NULL, datetime('now'), 'running', '{}', '{\"source\":\"native-linux-bridge\"}') "
      "ON CONFLICT(session_id) DO UPDATE SET active_app_id = excluded.active_app_id, status = 'running'";
  if (sqlite3_prepare_v2(bridge->storage->db, sql, -1, &statement, NULL) != SQLITE_OK) {
    return;
  }
  g_autofree gchar *session_id = runtime_session_id(request);
  bind_text(statement, 1, session_id);
  bind_text(statement, 2, request->context.app_id);
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

static void record_core_action(WebBridge *bridge, const gchar *event_id, const gchar *session_id, const gchar *app_id, JsonNode *action) {
  if (bridge == NULL || bridge->storage == NULL || bridge->storage->db == NULL) {
    return;
  }
  g_autofree gchar *action_id = new_core_id("core_action");
  g_autofree gchar *action_json = json_node_to_string_copy(action);
  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at) "
      "VALUES (?, ?, ?, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(bridge->storage->db, sql, -1, &statement, NULL) != SQLITE_OK) {
    return;
  }
  bind_text(statement, 1, action_id);
  bind_text(statement, 2, event_id);
  bind_text(statement, 3, session_id);
  bind_text(statement, 4, app_id);
  bind_text(statement, 5, action_json);
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

static void record_core_step(WebBridge *bridge, const BridgeRequest *request, JsonNode *response) {
  if (bridge == NULL || bridge->storage == NULL || bridge->storage->db == NULL ||
      request->context.app_id == NULL || g_strcmp0(request->method, "core.step") != 0 ||
      !json_object_has_member(request->params, "event")) {
    return;
  }
  JsonObject *response_object = response != NULL && JSON_NODE_HOLDS_OBJECT(response) ? json_node_get_object(response) : NULL;
  if (response_object == NULL || !json_object_get_boolean_member_with_default(response_object, "ok", FALSE)) {
    return;
  }
  JsonObject *result = json_object_get_object_member(response_object, "result");
  if (result == NULL) {
    return;
  }
  ensure_runtime_session(bridge, request);

  g_autofree gchar *event_id = new_core_id("core_event");
  g_autofree gchar *session_id = runtime_session_id(request);
  g_autofree gchar *event_json = json_node_to_string_copy(json_object_get_member(request->params, "event"));
  gint64 version_before = 0;
  gboolean has_version_before = state_version_before(result, &version_before);

  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at) "
      "VALUES (?, ?, ?, NULL, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(bridge->storage->db, sql, -1, &statement, NULL) != SQLITE_OK) {
    return;
  }
  bind_text(statement, 1, event_id);
  bind_text(statement, 2, session_id);
  bind_text(statement, 3, request->context.app_id);
  bind_nullable_int64(statement, 4, has_version_before, version_before);
  bind_text(statement, 5, event_json);
  gboolean inserted = sqlite3_step(statement) == SQLITE_DONE;
  sqlite3_finalize(statement);
  if (!inserted) {
    return;
  }

  JsonArray *actions = json_object_get_array_member(result, "actions");
  if (actions == NULL) {
    return;
  }
  for (guint index = 0; index < json_array_get_length(actions); ++index) {
    record_core_action(bridge, event_id, session_id, request->context.app_id, json_array_get_element(actions, index));
  }
}

static void record_bridge_call(WebBridge *bridge, const BridgeRequest *request, JsonNode *response, gint64 started_at_us) {
  if (bridge == NULL || bridge->storage == NULL || bridge->storage->db == NULL || request->context.app_id == NULL) {
    return;
  }
  ensure_runtime_session(bridge, request);

  JsonObject *response_object = response != NULL && JSON_NODE_HOLDS_OBJECT(response) ? json_node_get_object(response) : NULL;
  g_autofree gchar *bridge_call_id = new_bridge_call_id();
  g_autofree gchar *session_id = runtime_session_id(request);
  g_autofree gchar *params_json = json_object_to_string(request->params);
  g_autofree gchar *result_json = json_member_to_string(response_object, "result");
  g_autofree gchar *error_json = json_member_to_string(response_object, "error");

  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "INSERT INTO bridge_calls "
      "(bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) "
      "VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(bridge->storage->db, sql, -1, &statement, NULL) != SQLITE_OK) {
    return;
  }
  bind_text(statement, 1, bridge_call_id);
  bind_text(statement, 2, session_id);
  bind_text(statement, 3, request->context.app_id);
  bind_text(statement, 4, request->method);
  bind_text(statement, 5, params_json);
  bind_nullable_text(statement, 6, result_json);
  bind_nullable_text(statement, 7, error_json);
  sqlite3_bind_int64(statement, 8, (sqlite3_int64)((g_get_monotonic_time() - started_at_us) / 1000));
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

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

static JsonNode *capabilities_response(WebBridge *bridge, const BridgeRequest *request) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "platform");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, request->context.app_id != NULL ? request->context.app_id : "");
  json_builder_set_member_name(builder, "runtimeVersion");
  json_builder_add_string_value(builder, "0.1.0");
  json_builder_set_member_name(builder, "devMode");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "features");
  json_builder_begin_object(builder);
  const gchar *enabled[] = {"storage.read", "storage.write", "storage.get", "storage.set", "storage.remove", "storage.list", "notification.toast", "network.request", "runtime.capabilities", "app.log"};
  for (gsize index = 0; index < G_N_ELEMENTS(enabled); ++index) {
    json_builder_set_member_name(builder, enabled[index]);
    json_builder_add_boolean_value(builder, TRUE);
  }
  const gchar *dialog_features[] = {"dialog.openFile", "dialog.saveFile"};
  for (gsize index = 0; index < G_N_ELEMENTS(dialog_features); ++index) {
    json_builder_set_member_name(builder, dialog_features[index]);
    json_builder_add_boolean_value(builder, TRUE);
  }
  json_builder_set_member_name(builder, "core.step");
  json_builder_add_boolean_value(builder, zig_core_bridge_is_available(&bridge->core));
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
    return capabilities_response(bridge, request);
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

WebBridge *web_bridge_new(const gchar *database_path, GtkWindow *owner_window) {
  WebBridge *bridge = g_new0(WebBridge, 1);
  bridge->storage = platform_storage_new(database_path);
  platform_dialogs_init(&bridge->dialogs, owner_window);
  zig_core_bridge_init(&bridge->core);
  return bridge;
}

void web_bridge_free(WebBridge *bridge) {
  if (bridge == NULL) {
    return;
  }
  zig_core_bridge_clear(&bridge->core);
  platform_storage_free(bridge->storage);
  g_free(bridge);
}

gchar *web_bridge_handle_json(WebBridge *bridge, const gchar *body, AppSandboxContext context) {
  gint64 started_at_us = g_get_monotonic_time();
  BridgeRequest request = {.context = context};
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, body, -1, NULL)) {
    JsonNode *error = bridge_failure(NULL, "invalid_request", "Bridge message body must be JSON", NULL);
    gchar *text = bridge_response_to_string(error);
    json_node_unref(error);
    g_object_unref(parser);
    app_sandbox_context_clear(&request.context);
    return text;
  }

  JsonObject *root = json_node_get_object(json_parser_get_root(parser));
  if (json_object_has_member(root, "id")) {
    request.has_id = TRUE;
    request.id = g_strdup(json_object_get_string_member(root, "id"));
  }
  request.method = g_strdup(json_object_get_string_member_with_default(root, "method", ""));
  JsonObject *params = json_object_get_object_member(root, "params");
  request.params = params == NULL ? json_object_new() : json_object_ref(params);

  const gchar *permission = permission_for_bridge_method(request.method);
  if (permission != NULL && !approved_permissions_contains(&request.context, permission)) {
    JsonObject *details = json_object_new();
    json_object_set_string_member(details, "appId", request.context.app_id != NULL ? request.context.app_id : "");
    json_object_set_string_member(details, "method", request.method);
    json_object_set_string_member(details, "requiredPermission", permission);
    JsonNode *response = bridge_failure(&request, "permission_denied", "App cannot call requested bridge method", details);
    gchar *text = bridge_response_to_string(response);
    record_bridge_call(bridge, &request, response, started_at_us);
    json_node_unref(response);
    bridge_request_clear(&request);
    g_object_unref(parser);
    return text;
  }

  JsonNode *response = dispatch(bridge, &request);
  gchar *text = bridge_response_to_string(response);
  record_bridge_call(bridge, &request, response, started_at_us);
  record_core_step(bridge, &request, response);
  json_node_unref(response);
  bridge_request_clear(&request);
  g_object_unref(parser);
  return text;
}
