#include "webkit_host.h"

#include <json-glib/json-glib.h>

static const gchar *k_runtime_scheme = "app-runtime";

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

static gboolean on_script_message_with_reply(WebKitUserContentManager *manager, JSCValue *value, WebKitScriptMessageReply *reply, gpointer user_data) {
  (void)manager;
  WebKitHost *host = user_data;
  const gchar *uri = webkit_web_view_get_uri(host->web_view);
  g_autofree gchar *payload = jsc_value_to_json(value, 0);
  gchar *response = NULL;

  if (uri == NULL || !g_str_has_prefix(uri, "app-runtime://runtime-web/")) {
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
        g_autofree gchar *request_id = runtime_envelope_request_id(root);
        if (!has_valid_runtime_envelope(root)) {
          response = bridge_error_text(request_id, "invalid_request", "Runtime bridge envelope requires appId, mountToken, and request");
        } else {
          const gchar *app_id = json_object_get_string_member(root, "appId");
          const gchar *mount_token = json_object_get_string_member(root, "mountToken");
          if (!is_known_example_app_id(app_id)) {
            response = bridge_error_text(request_id, "invalid_request", "Runtime bridge envelope references an unknown app");
          } else {
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
