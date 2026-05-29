#include "webkit_host.h"

#include <json-glib/json-glib.h>

static const gchar *k_runtime_scheme = "app-runtime";

static AppSandboxContext sandbox_context_for_app(const gchar *app_id, const gchar *mount_token);

static gboolean logical_path_is_allowed(const gchar *path) {
  return path != NULL &&
         path[0] != '\0' &&
         strstr(path, "..") == NULL &&
         strchr(path, '\\') == NULL &&
         (g_str_has_prefix(path, "runtime/") || g_str_has_prefix(path, "webapps/examples/"));
}

static gchar *resource_path_for_logical_path(const gchar *root, const gchar *logical_path) {
  if (!logical_path_is_allowed(logical_path)) {
    return NULL;
  }
  if (g_str_has_prefix(logical_path, "runtime/")) {
    return g_build_filename(root, "runtime-web", logical_path + strlen("runtime/"), NULL);
  }
  return g_build_filename(root, logical_path, NULL);
}

static gchar *logical_path_for_runtime_uri(const gchar *uri) {
  if (!g_str_has_prefix(uri, "app-runtime://")) {
    return g_strdup("runtime/index.html");
  }

  const gchar *authority_and_path = uri + strlen("app-runtime://");
  const gchar *slash = strchr(authority_and_path, '/');
  g_autofree gchar *host = slash == NULL ? g_strdup(authority_and_path) : g_strndup(authority_and_path, slash - authority_and_path);
  const gchar *path = slash == NULL ? "" : slash + 1;

  if (g_strcmp0(host, "runtime") == 0) {
    if (path[0] == '\0' || g_strcmp0(path, "index.html") == 0) {
      return g_strdup("runtime/index.html");
    }
    return g_strdup(path);
  }

  if (path[0] == '\0') {
    return g_strdup(host);
  }
  return g_strdup_printf("%s/%s", host, path);
}

static const gchar *content_type_for_path(const gchar *path) {
  if (g_str_has_suffix(path, ".html")) {
    return "text/html";
  }
  if (g_str_has_suffix(path, ".css")) {
    return "text/css";
  }
  if (g_str_has_suffix(path, ".js")) {
    return "text/javascript";
  }
  if (g_str_has_suffix(path, ".json")) {
    return "application/json";
  }
  return "text/plain";
}

static gchar *repo_root(void) {
  g_autofree gchar *cwd = g_get_current_dir();
  gchar *current = g_strdup(cwd);
  for (int depth = 0; depth < 5; ++depth) {
    g_autofree gchar *prd = g_build_filename(current, "docs", "00_PRD.md", NULL);
    if (g_file_test(prd, G_FILE_TEST_EXISTS)) {
      return current;
    }
    gchar *parent = g_path_get_dirname(current);
    g_free(current);
    current = parent;
  }
  return g_strdup(cwd);
}

static gchar *database_path(void) {
  g_autofree gchar *data_dir = g_build_filename(g_get_user_data_dir(), "NativeAIWebappPlatform", NULL);
  g_mkdir_with_parents(data_dir, 0700);
  return g_build_filename(data_dir, "platform.sqlite", NULL);
}

static gchar *app_id_from_uri(const gchar *uri) {
  const gchar *markers[] = {"/webapps/examples/", "/examples/"};
  for (gsize index = 0; index < G_N_ELEMENTS(markers); ++index) {
    gchar *start = g_strstr_len(uri, -1, markers[index]);
    if (start == NULL) {
      continue;
    }
    start += strlen(markers[index]);
    gchar *end = strchr(start, '/');
    return end == NULL ? g_strdup(start) : g_strndup(start, end - start);
  }
  return g_strdup("unknown");
}

static gboolean is_known_example_app_id(const gchar *app_id) {
  const gchar *known[] = {"notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab"};
  for (gsize index = 0; index < G_N_ELEMENTS(known); ++index) {
    if (g_strcmp0(app_id, known[index]) == 0) {
      return TRUE;
    }
  }
  return FALSE;
}

static gboolean is_runtime_envelope(JsonObject *root) {
  return json_object_has_member(root, "appId") || json_object_has_member(root, "mountToken") || json_object_has_member(root, "request");
}

static gboolean is_trusted_runtime_uri(const gchar *uri) {
  return uri != NULL && g_str_has_prefix(uri, "app-runtime://runtime/");
}

static JsonObject *runtime_envelope_request(JsonObject *root) {
  JsonNode *request = json_object_get_member(root, "request");
  if (request == NULL || !JSON_NODE_HOLDS_OBJECT(request)) {
    return NULL;
  }
  return json_node_get_object(request);
}

static gchar *runtime_envelope_request_id(JsonObject *root) {
  JsonObject *request = runtime_envelope_request(root);
  if (request == NULL || !json_object_has_member(request, "id")) {
    return NULL;
  }
  return g_strdup(json_object_get_string_member(request, "id"));
}

static gboolean has_valid_runtime_envelope(JsonObject *root) {
  const gchar *app_id = json_object_get_string_member_with_default(root, "appId", "");
  const gchar *mount_token = json_object_get_string_member_with_default(root, "mountToken", "");
  return app_id[0] != '\0' && mount_token[0] != '\0' && runtime_envelope_request(root) != NULL;
}

static gchar *json_node_to_string(JsonNode *node) {
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, node);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  return text;
}

static gchar *bridge_error_text(const gchar *request_id, const gchar *code, const gchar *message) {
  BridgeRequest request = {
      .id = (gchar *)request_id,
      .has_id = request_id != NULL && request_id[0] != '\0',
  };
  JsonNode *response = bridge_failure(&request, code, message, NULL);
  return bridge_response_to_string(response);
}

static gboolean json_response_ok(const gchar *text) {
  JsonParser *parser = json_parser_new();
  gboolean ok = FALSE;
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonObject *root = json_node_get_object(json_parser_get_root(parser));
    ok = json_object_get_boolean_member_with_default(root, "ok", FALSE);
  }
  g_object_unref(parser);
  return ok;
}

static gboolean storage_smoke_response_matches(const gchar *text, const gchar *value) {
  JsonParser *parser = json_parser_new();
  gboolean matches = FALSE;
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonObject *root = json_node_get_object(json_parser_get_root(parser));
    JsonObject *result = json_object_get_object_member(root, "result");
    JsonObject *stored = result == NULL ? NULL : json_object_get_object_member(result, "value");
    matches = stored != NULL && g_strcmp0(json_object_get_string_member_with_default(stored, "smokeValue", ""), value) == 0;
  }
  g_object_unref(parser);
  return matches;
}

static gboolean storage_list_response_contains(const gchar *text, const gchar *key) {
  JsonParser *parser = json_parser_new();
  gboolean matches = FALSE;
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonObject *root = json_node_get_object(json_parser_get_root(parser));
    JsonObject *result = json_object_get_object_member(root, "result");
    JsonArray *keys = result == NULL ? NULL : json_object_get_array_member(result, "keys");
    if (keys != NULL) {
      guint length = json_array_get_length(keys);
      for (guint index = 0; index < length; ++index) {
        if (g_strcmp0(json_array_get_string_element(keys, index), key) == 0) {
          matches = TRUE;
          break;
        }
      }
    }
  }
  g_object_unref(parser);
  return matches;
}

static gboolean storage_get_response_is_null(const gchar *text) {
  JsonParser *parser = json_parser_new();
  gboolean matches = FALSE;
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonObject *root = json_node_get_object(json_parser_get_root(parser));
    JsonObject *result = json_object_get_object_member(root, "result");
    JsonNode *value = result == NULL ? NULL : json_object_get_member(result, "value");
    matches = value != NULL && json_node_get_node_type(value) == JSON_NODE_NULL;
  }
  g_object_unref(parser);
  return matches;
}

static gboolean json_response_error_code_matches(const gchar *text, const gchar *code) {
  JsonParser *parser = json_parser_new();
  gboolean matches = FALSE;
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonObject *root = json_node_get_object(json_parser_get_root(parser));
    JsonObject *error = json_object_get_object_member(root, "error");
    matches = error != NULL && g_strcmp0(json_object_get_string_member_with_default(error, "code", ""), code) == 0;
  }
  g_object_unref(parser);
  return matches;
}

static gchar *request_json(const gchar *id, const gchar *method, JsonNode *params) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "id");
  json_builder_add_string_value(builder, id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, method);
  json_builder_set_member_name(builder, "params");
  json_builder_add_value(builder, json_node_copy(params));
  json_builder_end_object(builder);

  JsonGenerator *generator = json_generator_new();
  JsonNode *root = json_builder_get_root(builder);
  json_generator_set_root(generator, root);
  gchar *text = json_generator_to_data(generator, NULL);
  json_node_unref(root);
  g_object_unref(generator);
  g_object_unref(builder);
  return text;
}

static gchar *runtime_envelope_json(const gchar *app_id, const gchar *mount_token, const gchar *id, const gchar *method, JsonNode *params) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "mountToken");
  json_builder_add_string_value(builder, mount_token);
  json_builder_set_member_name(builder, "request");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "id");
  json_builder_add_string_value(builder, id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, method);
  json_builder_set_member_name(builder, "params");
  json_builder_add_value(builder, json_node_copy(params));
  json_builder_end_object(builder);
  json_builder_end_object(builder);

  JsonGenerator *generator = json_generator_new();
  JsonNode *root = json_builder_get_root(builder);
  json_generator_set_root(generator, root);
  gchar *text = json_generator_to_data(generator, NULL);
  json_node_unref(root);
  g_object_unref(generator);
  g_object_unref(builder);
  return text;
}

static gchar *bridge_call(WebKitHost *host, const gchar *app_id, const gchar *id, const gchar *method, JsonNode *params) {
  g_autofree gchar *body = request_json(id, method, params);
  AppSandboxContext context = sandbox_context_for_app(app_id, "linux-smoke");
  return web_bridge_handle_json(host->bridge, body, context);
}

static void finish_smoke(WebKitHost *host) {
  if (g_strcmp0(g_getenv("NATIVE_AI_LINUX_SMOKE_EXIT_AFTER"), "1") == 0) {
    g_application_quit(G_APPLICATION(host->application));
  }
}

static void smoke_failure(WebKitHost *host, const gchar *message) {
  g_printerr("NATIVE_AI_LINUX_SMOKE_FAILED: %s\n", message);
  finish_smoke(host);
}

static void smoke_success(WebKitHost *host, const gchar *marker) {
  g_print("%s\n", marker);
  finish_smoke(host);
}

static gint bridge_log_count(WebKitHost *host, const gchar *app_id, const gchar *method) {
  if (host == NULL || host->bridge == NULL || host->bridge->storage == NULL || host->bridge->storage->db == NULL) {
    return 0;
  }
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          host->bridge->storage->db,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ?",
          -1,
          &statement,
          NULL) != SQLITE_OK) {
    return 0;
  }
  sqlite3_bind_text(statement, 1, app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, method, -1, SQLITE_TRANSIENT);
  gint count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

static void run_storage_smoke(WebKitHost *host, gboolean set_value) {
  const gchar *key = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_KEY");
  const gchar *value = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
  if (key == NULL || value == NULL) {
    smoke_failure(host, "storage smoke requires NATIVE_AI_LINUX_SMOKE_STORAGE_KEY and NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
    return;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "key");
  json_builder_add_string_value(builder, key);
  if (set_value) {
    json_builder_set_member_name(builder, "value");
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "smokeValue");
    json_builder_add_string_value(builder, value);
    json_builder_end_object(builder);
  }
  json_builder_end_object(builder);
  JsonNode *params = json_builder_get_root(builder);
  g_autofree gchar *response = bridge_call(host, "notes-lite", set_value ? "linux_smoke_storage_set" : "linux_smoke_storage_get", set_value ? "storage.set" : "storage.get", params);
  json_node_unref(params);
  g_object_unref(builder);

  if (!json_response_ok(response)) {
    smoke_failure(host, response);
    return;
  }

  if (!set_value) {
    JsonParser *parser = json_parser_new();
    gboolean matches = FALSE;
    if (json_parser_load_from_data(parser, response, -1, NULL)) {
      JsonObject *root = json_node_get_object(json_parser_get_root(parser));
      JsonObject *result = json_object_get_object_member(root, "result");
      JsonObject *stored = result == NULL ? NULL : json_object_get_object_member(result, "value");
      matches = stored != NULL && g_strcmp0(json_object_get_string_member_with_default(stored, "smokeValue", ""), value) == 0;
    }
    g_object_unref(parser);
    if (!matches) {
      smoke_failure(host, response);
      return;
    }
  }

  smoke_success(host, set_value ? "NATIVE_AI_LINUX_SMOKE_STORAGE_SET_OK" : "NATIVE_AI_LINUX_SMOKE_STORAGE_GET_OK");
}

static void run_core_smoke(WebKitHost *host) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "event");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "type");
  json_builder_add_string_value(builder, "CreateTask");
  json_builder_set_member_name(builder, "payload");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "title");
  json_builder_add_string_value(builder, "Linux smoke task");
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  JsonNode *params = json_builder_get_root(builder);
  g_autofree gchar *response = bridge_call(host, "task-workbench", "linux_smoke_core_step", "core.step", params);
  json_node_unref(params);
  g_object_unref(builder);

  if (!json_response_ok(response)) {
    smoke_failure(host, response);
    return;
  }
  smoke_success(host, "NATIVE_AI_LINUX_SMOKE_CORE_STEP_OK");
}

static gboolean require_smoke_ok(WebKitHost *host, const gchar *response) {
  if (!json_response_ok(response)) {
    smoke_failure(host, response);
    return FALSE;
  }
  return TRUE;
}

static void run_fixed_bridge_surface_smoke(WebKitHost *host) {
  const gchar *key = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_KEY");
  const gchar *value = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
  if (key == NULL || value == NULL) {
    smoke_failure(host, "fixed bridge surface smoke requires NATIVE_AI_LINUX_SMOKE_STORAGE_KEY and NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
    return;
  }

  JsonBuilder *set_builder = json_builder_new();
  json_builder_begin_object(set_builder);
  json_builder_set_member_name(set_builder, "key");
  json_builder_add_string_value(set_builder, key);
  json_builder_set_member_name(set_builder, "value");
  json_builder_begin_object(set_builder);
  json_builder_set_member_name(set_builder, "smokeValue");
  json_builder_add_string_value(set_builder, value);
  json_builder_end_object(set_builder);
  json_builder_end_object(set_builder);
  JsonNode *set_params = json_builder_get_root(set_builder);
  g_autofree gchar *set_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_storage_set", "storage.set", set_params);
  json_node_unref(set_params);
  g_object_unref(set_builder);
  if (!require_smoke_ok(host, set_response)) {
    return;
  }

  JsonBuilder *list_builder = json_builder_new();
  json_builder_begin_object(list_builder);
  json_builder_set_member_name(list_builder, "prefix");
  json_builder_add_string_value(list_builder, "notes-lite:");
  json_builder_end_object(list_builder);
  JsonNode *list_params = json_builder_get_root(list_builder);
  g_autofree gchar *list_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_storage_list", "storage.list", list_params);
  json_node_unref(list_params);
  g_object_unref(list_builder);
  if (!json_response_ok(list_response) || !storage_list_response_contains(list_response, key)) {
    smoke_failure(host, list_response);
    return;
  }

  JsonBuilder *remove_builder = json_builder_new();
  json_builder_begin_object(remove_builder);
  json_builder_set_member_name(remove_builder, "key");
  json_builder_add_string_value(remove_builder, key);
  json_builder_end_object(remove_builder);
  JsonNode *remove_params = json_builder_get_root(remove_builder);
  g_autofree gchar *remove_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_storage_remove", "storage.remove", remove_params);
  json_node_unref(remove_params);
  g_object_unref(remove_builder);
  if (!require_smoke_ok(host, remove_response)) {
    return;
  }

  JsonBuilder *get_builder = json_builder_new();
  json_builder_begin_object(get_builder);
  json_builder_set_member_name(get_builder, "key");
  json_builder_add_string_value(get_builder, key);
  json_builder_end_object(get_builder);
  JsonNode *get_params = json_builder_get_root(get_builder);
  g_autofree gchar *get_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_storage_get_removed", "storage.get", get_params);
  json_node_unref(get_params);
  g_object_unref(get_builder);
  if (!json_response_ok(get_response) || !storage_get_response_is_null(get_response)) {
    smoke_failure(host, get_response);
    return;
  }

  JsonBuilder *notification_builder = json_builder_new();
  json_builder_begin_object(notification_builder);
  json_builder_set_member_name(notification_builder, "title");
  json_builder_add_string_value(notification_builder, "Native AI smoke");
  json_builder_set_member_name(notification_builder, "body");
  json_builder_add_string_value(notification_builder, "Fixed bridge surface smoke");
  json_builder_end_object(notification_builder);
  JsonNode *notification_params = json_builder_get_root(notification_builder);
  g_autofree gchar *notification_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_notification", "notification.toast", notification_params);
  json_node_unref(notification_params);
  g_object_unref(notification_builder);
  if (!require_smoke_ok(host, notification_response)) {
    return;
  }

  JsonBuilder *log_builder = json_builder_new();
  json_builder_begin_object(log_builder);
  json_builder_set_member_name(log_builder, "level");
  json_builder_add_string_value(log_builder, "info");
  json_builder_set_member_name(log_builder, "message");
  json_builder_add_string_value(log_builder, "Fixed bridge surface smoke");
  json_builder_end_object(log_builder);
  JsonNode *log_params = json_builder_get_root(log_builder);
  g_autofree gchar *log_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_app_log", "app.log", log_params);
  json_node_unref(log_params);
  g_object_unref(log_builder);
  if (!require_smoke_ok(host, log_response)) {
    return;
  }

  JsonBuilder *capabilities_builder = json_builder_new();
  json_builder_begin_object(capabilities_builder);
  json_builder_end_object(capabilities_builder);
  JsonNode *capabilities_params = json_builder_get_root(capabilities_builder);
  g_autofree gchar *capabilities_response = bridge_call(host, "notes-lite", "linux_smoke_fixed_capabilities", "runtime.capabilities", capabilities_params);
  json_node_unref(capabilities_params);
  g_object_unref(capabilities_builder);
  if (!require_smoke_ok(host, capabilities_response)) {
    return;
  }

  JsonBuilder *network_builder = json_builder_new();
  json_builder_begin_object(network_builder);
  json_builder_set_member_name(network_builder, "url");
  json_builder_add_string_value(network_builder, "https://blocked.example.com/status");
  json_builder_set_member_name(network_builder, "method");
  json_builder_add_string_value(network_builder, "GET");
  json_builder_end_object(network_builder);
  JsonNode *network_params = json_builder_get_root(network_builder);
  g_autofree gchar *network_response = bridge_call(host, "api-dashboard", "linux_smoke_fixed_network_denied", "network.request", network_params);
  json_node_unref(network_params);
  g_object_unref(network_builder);
  if (!json_response_error_code_matches(network_response, "network_policy_denied")) {
    smoke_failure(host, network_response);
    return;
  }

  if (bridge_log_count(host, "notes-lite", "storage.set") <= 0 || bridge_log_count(host, "api-dashboard", "network.request") <= 0) {
    smoke_failure(host, "fixed bridge surface smoke did not persist bridge_calls rows");
    return;
  }

  smoke_success(host, "NATIVE_AI_LINUX_SMOKE_FIXED_BRIDGE_SURFACE_OK");
}

static void maybe_finish_web_bridge_smoke(WebKitHost *host, const gchar *request_id, const gchar *response) {
  if (request_id == NULL || response == NULL) {
    return;
  }
  if (g_strcmp0(request_id, "linux_smoke_bridge_storage_set") == 0) {
    json_response_ok(response) ? smoke_success(host, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK") : smoke_failure(host, response);
  } else if (g_strcmp0(request_id, "linux_smoke_bridge_storage_get") == 0) {
    const gchar *value = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
    json_response_ok(response) && storage_smoke_response_matches(response, value)
        ? smoke_success(host, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK")
        : smoke_failure(host, response);
  } else if (g_strcmp0(request_id, "linux_smoke_bridge_core_step") == 0) {
    json_response_ok(response) ? smoke_success(host, "NATIVE_AI_LINUX_SMOKE_BRIDGE_CORE_STEP_OK") : smoke_failure(host, response);
  }
}

static void maybe_finish_runtime_app_bridge_smoke(WebKitHost *host, const gchar *app_id, const gchar *method, const gchar *response) {
  if (g_strcmp0(g_getenv("NATIVE_AI_LINUX_SMOKE"), "runtime-app-storage-get") != 0) {
    return;
  }
  if (g_strcmp0(app_id, "notes-lite") == 0 && g_strcmp0(method, "storage.get") == 0) {
    json_response_ok(response)
        ? smoke_success(host, "NATIVE_AI_LINUX_SMOKE_RUNTIME_APP_STORAGE_GET_OK")
        : smoke_failure(host, response);
  }
}

static void smoke_script_done(GObject *source_object, GAsyncResult *result, gpointer user_data) {
  WebKitHost *host = user_data;
  GError *error = NULL;
  JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source_object), result, &error);
  if (error != NULL) {
    smoke_failure(host, error->message);
    g_error_free(error);
    return;
  }
  g_autofree gchar *status = value == NULL ? g_strdup("") : jsc_value_to_string(value);
  if (g_strcmp0(status, "posted") != 0 && g_strcmp0(status, "started") != 0) {
    smoke_failure(host, status);
  }
  g_clear_object(&value);
}

static void run_runtime_app_bridge_smoke(WebKitHost *host) {
  const gchar *script =
      "(function () {"
      "var deadline = Date.now() + 5000;"
      "function openNotesLiteWhenReady() {"
      "var button = document.querySelector('[data-testid=\"open-notes-lite-button\"]');"
      "if (button) { button.click(); return; }"
      "if (Date.now() < deadline) window.setTimeout(openNotesLiteWhenReady, 50);"
      "}"
      "openNotesLiteWhenReady();"
      "return 'started';"
      "})()";
  webkit_web_view_evaluate_javascript(host->web_view, script, -1, NULL, NULL, NULL, smoke_script_done, host);
}

static void start_web_bridge_smoke(WebKitHost *host, const gchar *app_id, const gchar *id, const gchar *method, JsonNode *params) {
  g_autofree gchar *envelope = runtime_envelope_json(app_id, "linux-webkit-smoke", id, method, params);
  g_autofree gchar *script = g_strdup_printf(
      "(function () {"
      "var handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.NativeAIPlatformBridge;"
      "if (!handler || typeof handler.postMessage !== 'function') return 'missing-webkit-bridge';"
      "handler.postMessage(%s);"
      "return 'posted';"
      "})()",
      envelope);
  webkit_web_view_evaluate_javascript(host->web_view, script, -1, NULL, NULL, NULL, smoke_script_done, host);
}

static void run_web_bridge_storage_smoke(WebKitHost *host, gboolean set_value) {
  const gchar *key = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_KEY");
  const gchar *value = g_getenv("NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
  if (key == NULL || value == NULL) {
    smoke_failure(host, "web bridge storage smoke requires NATIVE_AI_LINUX_SMOKE_STORAGE_KEY and NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE");
    return;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "key");
  json_builder_add_string_value(builder, key);
  if (set_value) {
    json_builder_set_member_name(builder, "value");
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "smokeValue");
    json_builder_add_string_value(builder, value);
    json_builder_end_object(builder);
  }
  json_builder_end_object(builder);
  JsonNode *params = json_builder_get_root(builder);
  start_web_bridge_smoke(host, "notes-lite", set_value ? "linux_smoke_bridge_storage_set" : "linux_smoke_bridge_storage_get", set_value ? "storage.set" : "storage.get", params);
  json_node_unref(params);
  g_object_unref(builder);
}

static void run_web_bridge_core_smoke(WebKitHost *host) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "event");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "type");
  json_builder_add_string_value(builder, "CreateTask");
  json_builder_set_member_name(builder, "payload");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "title");
  json_builder_add_string_value(builder, "Linux WebKit bridge smoke task");
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  JsonNode *params = json_builder_get_root(builder);
  start_web_bridge_smoke(host, "task-workbench", "linux_smoke_bridge_core_step", "core.step", params);
  json_node_unref(params);
  g_object_unref(builder);
}

static void run_smoke(WebKitHost *host) {
  if (host->smoke_ran) {
    return;
  }
  const gchar *action = g_getenv("NATIVE_AI_LINUX_SMOKE");
  if (action == NULL || action[0] == '\0') {
    return;
  }
  host->smoke_ran = TRUE;
  g_print("NATIVE_AI_LINUX_SMOKE_STARTED_%s\n", action);
  if (g_strcmp0(action, "runtime-load") == 0) {
    smoke_success(host, "NATIVE_AI_LINUX_SMOKE_RUNTIME_LOADED");
  } else if (g_strcmp0(action, "storage-set") == 0) {
    run_storage_smoke(host, TRUE);
  } else if (g_strcmp0(action, "storage-get") == 0) {
    run_storage_smoke(host, FALSE);
  } else if (g_strcmp0(action, "core-step") == 0) {
    run_core_smoke(host);
  } else if (g_strcmp0(action, "fixed-bridge-surface") == 0) {
    run_fixed_bridge_surface_smoke(host);
  } else if (g_strcmp0(action, "bridge-storage-set") == 0) {
    run_web_bridge_storage_smoke(host, TRUE);
  } else if (g_strcmp0(action, "bridge-storage-get") == 0) {
    run_web_bridge_storage_smoke(host, FALSE);
  } else if (g_strcmp0(action, "bridge-core-step") == 0) {
    run_web_bridge_core_smoke(host);
  } else if (g_strcmp0(action, "runtime-app-storage-get") == 0) {
    run_runtime_app_bridge_smoke(host);
  } else {
    smoke_failure(host, "unknown smoke action");
  }
}

static GHashTable *permissions_for_app(const gchar *app_id) {
  GHashTable *permissions = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
  g_autofree gchar *root = repo_root();
  g_autofree gchar *manifest_path = g_build_filename(root, "webapps", "examples", app_id, "manifest.json", NULL);
  g_autofree gchar *contents = NULL;
  if (!g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    return permissions;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    return permissions;
  }
  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  JsonArray *array = json_object_get_array_member(manifest, "permissions");
  if (array != NULL) {
    guint length = json_array_get_length(array);
    for (guint index = 0; index < length; ++index) {
      g_hash_table_add(permissions, g_strdup(json_array_get_string_element(array, index)));
    }
  }
  g_object_unref(parser);
  return permissions;
}

static GPtrArray *network_policy_for_app(const gchar *app_id) {
  GPtrArray *rules = g_ptr_array_new_with_free_func(network_policy_rule_free);
  g_autofree gchar *root = repo_root();
  g_autofree gchar *manifest_path = g_build_filename(root, "webapps", "examples", app_id, "manifest.json", NULL);
  g_autofree gchar *contents = NULL;
  if (!g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    return rules;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    return rules;
  }
  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  JsonObject *policy = json_object_get_object_member(manifest, "networkPolicy");
  JsonArray *allow = policy == NULL ? NULL : json_object_get_array_member(policy, "allow");
  if (allow != NULL) {
    guint length = json_array_get_length(allow);
    for (guint index = 0; index < length; ++index) {
      JsonObject *raw = json_array_get_object_element(allow, index);
      if (raw == NULL || !json_object_has_member(raw, "origin")) {
        continue;
      }
      NetworkPolicyRule *rule = g_new0(NetworkPolicyRule, 1);
      rule->origin = g_strdup(json_object_get_string_member(raw, "origin"));
      rule->methods = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
      rule->allowed_headers = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
      rule->max_request_bytes = (gsize)json_object_get_int_member_with_default(raw, "maxRequestBytes", 0);
      rule->max_response_bytes = (gsize)json_object_get_int_member_with_default(raw, "maxResponseBytes", 0);
      rule->timeout_ms = (guint)json_object_get_int_member_with_default(raw, "timeoutMs", 10000);

      JsonArray *methods = json_object_get_array_member(raw, "methods");
      if (methods != NULL) {
        for (guint method_index = 0; method_index < json_array_get_length(methods); ++method_index) {
          g_hash_table_add(rule->methods, g_ascii_strup(json_array_get_string_element(methods, method_index), -1));
        }
      }
      JsonArray *headers = json_object_get_array_member(raw, "allowedHeaders");
      if (headers != NULL) {
        for (guint header_index = 0; header_index < json_array_get_length(headers); ++header_index) {
          g_hash_table_add(rule->allowed_headers, g_ascii_strdown(json_array_get_string_element(headers, header_index), -1));
        }
      }
      g_ptr_array_add(rules, rule);
    }
  }
  g_object_unref(parser);
  return rules;
}

static gboolean deny_private_network_for_app(const gchar *app_id) {
  g_autofree gchar *root = repo_root();
  g_autofree gchar *manifest_path = g_build_filename(root, "webapps", "examples", app_id, "manifest.json", NULL);
  g_autofree gchar *contents = NULL;
  if (!g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    return TRUE;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    return TRUE;
  }
  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  JsonObject *policy = json_object_get_object_member(manifest, "networkPolicy");
  gboolean deny = TRUE;
  if (policy != NULL && json_object_has_member(policy, "denyPrivateNetwork")) {
    JsonNode *value = json_object_get_member(policy, "denyPrivateNetwork");
    if (json_node_get_value_type(value) == G_TYPE_BOOLEAN) {
      deny = json_node_get_boolean(value);
    }
  }
  g_object_unref(parser);
  return deny;
}

static AppSandboxContext sandbox_context_from_uri(const gchar *uri) {
  g_autofree gchar *app_id = app_id_from_uri(uri);
  return (AppSandboxContext){
      .app_id = g_strdup(app_id),
      .storage_prefix = g_strdup_printf("%s:", app_id),
      .approved_permissions = permissions_for_app(app_id),
      .network_policy = network_policy_for_app(app_id),
      .deny_private_network = deny_private_network_for_app(app_id),
      .mount_token = NULL,
  };
}

static AppSandboxContext sandbox_context_for_app(const gchar *app_id, const gchar *mount_token) {
  return (AppSandboxContext){
      .app_id = g_strdup(app_id),
      .storage_prefix = g_strdup_printf("%s:", app_id),
      .approved_permissions = permissions_for_app(app_id),
      .network_policy = network_policy_for_app(app_id),
      .deny_private_network = deny_private_network_for_app(app_id),
      .mount_token = g_strdup(mount_token),
  };
}

static void runtime_scheme_cb(WebKitURISchemeRequest *request, gpointer user_data) {
  (void)user_data;
  const gchar *uri = webkit_uri_scheme_request_get_uri(request);
  g_autofree gchar *root = repo_root();
  g_autofree gchar *logical_path = logical_path_for_runtime_uri(uri);
  g_autofree gchar *file_path = resource_path_for_logical_path(root, logical_path);
  if (file_path == NULL) {
    webkit_uri_scheme_request_finish_error(request, g_error_new(G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "Runtime resource was not found"));
    return;
  }
  GFile *file = g_file_new_for_path(file_path);
  GFileInputStream *stream = g_file_read(file, NULL, NULL);
  if (stream == NULL) {
    webkit_uri_scheme_request_finish_error(request, g_error_new(G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "Runtime resource was not found"));
    g_clear_object(&file);
    return;
  }
  webkit_uri_scheme_request_finish(request, G_INPUT_STREAM(stream), -1, content_type_for_path(logical_path));
  g_clear_object(&stream);
  g_clear_object(&file);
}

static void on_load_changed(WebKitWebView *web_view, WebKitLoadEvent load_event, gpointer user_data) {
  if (load_event != WEBKIT_LOAD_FINISHED) {
    return;
  }
  WebKitHost *host = user_data;
  const gchar *uri = webkit_web_view_get_uri(web_view);
  if (g_strcmp0(uri, "app-runtime://runtime/index.html") == 0) {
    run_smoke(host);
  }
}

static gboolean on_script_message_with_reply(WebKitUserContentManager *manager, JSCValue *value, WebKitScriptMessageReply *reply, gpointer user_data) {
  (void)manager;
  WebKitHost *host = user_data;
  const gchar *uri = webkit_web_view_get_uri(host->web_view);
  g_autofree gchar *payload = jsc_value_to_json(value, 0);
  g_autofree gchar *request_id = NULL;
  g_autofree gchar *request_app_id = NULL;
  g_autofree gchar *request_method = NULL;
  gchar *response = NULL;

  if (!is_trusted_runtime_uri(uri)) {
    response = bridge_error_text(NULL, "bridge.unauthorized_channel", "Runtime bridge envelope must come from the trusted runtime view");
  } else if (payload == NULL) {
    response = bridge_error_text(NULL, "invalid_request", "Runtime bridge envelope must be JSON");
  } else {
    JsonParser *parser = json_parser_new();
    if (!json_parser_load_from_data(parser, payload, -1, NULL)) {
      response = bridge_error_text(NULL, "invalid_request", "Runtime bridge envelope must be JSON");
    } else {
      JsonNode *root_node = json_parser_get_root(parser);
      if (root_node == NULL || !JSON_NODE_HOLDS_OBJECT(root_node)) {
        response = bridge_error_text(NULL, "invalid_request", "Runtime bridge envelope must be an object");
      } else if (is_runtime_envelope(json_node_get_object(root_node))) {
        JsonObject *root = json_node_get_object(root_node);
        request_id = runtime_envelope_request_id(root);
        if (!has_valid_runtime_envelope(root)) {
          response = bridge_error_text(request_id, "invalid_request", "Runtime bridge envelope requires appId, mountToken, and request");
        } else {
          const gchar *app_id = json_object_get_string_member(root, "appId");
          const gchar *mount_token = json_object_get_string_member(root, "mountToken");
          if (!is_known_example_app_id(app_id)) {
            response = bridge_error_text(request_id, "invalid_request", "Runtime bridge envelope references an unknown app");
          } else {
            JsonObject *request = runtime_envelope_request(root);
            request_app_id = g_strdup(app_id);
            request_method = g_strdup(json_object_get_string_member_with_default(request, "method", ""));
            JsonNode *request_node = json_object_get_member(root, "request");
            g_autofree gchar *request_body = json_node_to_string(request_node);
            AppSandboxContext context = sandbox_context_for_app(app_id, mount_token);
            response = web_bridge_handle_json(host->bridge, request_body, context);
          }
        }
      } else {
        AppSandboxContext context = sandbox_context_from_uri(uri);
        response = web_bridge_handle_json(host->bridge, payload, context);
      }
    }
    g_object_unref(parser);
  }

  maybe_finish_web_bridge_smoke(host, request_id, response);
  maybe_finish_runtime_app_bridge_smoke(host, request_app_id, request_method, response);
  JSCContext *js_context = jsc_value_get_context(value);
  JSCValue *reply_value = jsc_value_new_from_json(js_context, response);
  webkit_script_message_reply_return_value(reply, reply_value);
  g_object_unref(reply_value);
  g_free(response);
  return TRUE;
}

WebKitHost *webkit_host_new(GtkApplication *application) {
  WebKitHost *host = g_new0(WebKitHost, 1);
  host->application = application;
  host->window = gtk_application_window_new(application);
  gtk_window_set_title(GTK_WINDOW(host->window), "Native AI Webapp Platform");
  gtk_window_set_default_size(GTK_WINDOW(host->window), 1200, 820);

  WebKitWebContext *context = webkit_web_context_get_default();
  WebKitSecurityManager *security_manager = webkit_web_context_get_security_manager(context);
  webkit_security_manager_register_uri_scheme_as_secure(security_manager, k_runtime_scheme);
  webkit_web_context_register_uri_scheme(context, k_runtime_scheme, runtime_scheme_cb, NULL, NULL);

  WebKitUserContentManager *content_manager = webkit_user_content_manager_new();
  webkit_user_content_manager_register_script_message_handler_with_reply(content_manager, "NativeAIPlatformBridge", NULL);
  g_signal_connect(content_manager, "script-message-with-reply-received::NativeAIPlatformBridge", G_CALLBACK(on_script_message_with_reply), host);

  host->web_view = WEBKIT_WEB_VIEW(webkit_web_view_new_with_user_content_manager(content_manager));
  g_signal_connect(host->web_view, "load-changed", G_CALLBACK(on_load_changed), host);
  gtk_window_set_child(GTK_WINDOW(host->window), GTK_WIDGET(host->web_view));
  g_autofree gchar *db_path = database_path();
  host->bridge = web_bridge_new(db_path, GTK_WINDOW(host->window));
  webkit_web_view_load_uri(host->web_view, "app-runtime://runtime/index.html");
  return host;
}

void webkit_host_present(WebKitHost *host) {
  gtk_window_present(GTK_WINDOW(host->window));
}

void webkit_host_free(WebKitHost *host) {
  if (host == NULL) {
    return;
  }
  web_bridge_free(host->bridge);
  g_free(host);
}
