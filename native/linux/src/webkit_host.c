#include "webkit_host.h"

#include <json-glib/json-glib.h>

static const gchar *k_runtime_scheme = "app-runtime";

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

static AppSandboxContext sandbox_context_from_uri(const gchar *uri) {
  gchar *app_id = app_id_from_uri(uri);
  return (AppSandboxContext){
      .app_id = g_strdup(app_id),
      .storage_prefix = g_strdup_printf("%s:", app_id),
      .approved_permissions = permissions_for_app(app_id),
      .network_policy = network_policy_for_app(app_id),
  };
}

static void runtime_scheme_cb(WebKitURISchemeRequest *request, gpointer user_data) {
  (void)user_data;
  const gchar *uri = webkit_uri_scheme_request_get_uri(request);
  g_autofree gchar *root = repo_root();
  const gchar *path = g_str_has_prefix(uri, "app-runtime://") ? uri + strlen("app-runtime://") : "runtime/index.html";
  g_autofree gchar *file_path = g_build_filename(root, path, NULL);
  GFile *file = g_file_new_for_path(file_path);
  GFileInputStream *stream = g_file_read(file, NULL, NULL);
  webkit_uri_scheme_request_finish(request, G_INPUT_STREAM(stream), -1, "text/html");
  g_clear_object(&stream);
  g_clear_object(&file);
}

static void on_script_message(WebKitUserContentManager *manager, WebKitJavascriptResult *result, gpointer user_data) {
  (void)manager;
  WebKitHost *host = user_data;
  JSCValue *value = webkit_javascript_result_get_js_value(result);
  g_autofree gchar *payload = jsc_value_to_string(value);
  const gchar *uri = webkit_web_view_get_uri(host->web_view);
  AppSandboxContext context = sandbox_context_from_uri(uri == NULL ? "" : uri);
  gchar *response = web_bridge_handle_json(host->bridge, payload, context);
  webkit_web_view_evaluate_javascript(host->web_view, response, -1, NULL, NULL, NULL, NULL, NULL);
  g_free(response);
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
  webkit_user_content_manager_register_script_message_handler(content_manager, "NativeAIPlatformBridge");
  g_signal_connect(content_manager, "script-message-received::NativeAIPlatformBridge", G_CALLBACK(on_script_message), host);

  host->web_view = WEBKIT_WEB_VIEW(webkit_web_view_new_with_user_content_manager(content_manager));
  gtk_window_set_child(GTK_WINDOW(host->window), GTK_WIDGET(host->web_view));
  g_autofree gchar *db_path = database_path();
  host->bridge = web_bridge_new(db_path);
  webkit_web_view_load_uri(host->web_view, "app-runtime://runtime-web/index.html");
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
