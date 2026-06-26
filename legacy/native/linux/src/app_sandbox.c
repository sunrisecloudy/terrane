#include "app_sandbox.h"

#include <json-glib/json-glib.h>
#include <string.h>

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

static gchar *executable_dir(void) {
  g_autofree gchar *target = g_file_read_link("/proc/self/exe", NULL);
  if (target != NULL) {
    return g_path_get_dirname(target);
  }
  return g_get_current_dir();
}

static gchar *packaged_resource_path_for_logical_path(const gchar *logical_path) {
  g_autofree gchar *dir = executable_dir();
  if (g_str_has_prefix(logical_path, "webapps/examples/")) {
    return g_build_filename(dir, "resources", "webapps", "examples", logical_path + strlen("webapps/examples/"), NULL);
  }
  return NULL;
}

static gboolean app_id_is_safe_path_segment(const gchar *app_id) {
  return app_id != NULL &&
         app_id[0] != '\0' &&
         strstr(app_id, "..") == NULL &&
         strchr(app_id, '/') == NULL &&
         strchr(app_id, '\\') == NULL;
}

gboolean app_sandbox_is_known_example_app_id(const gchar *app_id) {
  const gchar *known[] = {"notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab", "calendar-planner"};
  for (gsize index = 0; index < G_N_ELEMENTS(known); ++index) {
    if (g_strcmp0(app_id, known[index]) == 0) {
      return TRUE;
    }
  }
  return FALSE;
}

gchar *app_sandbox_manifest_path_for_app(const gchar *app_id) {
  if (!app_id_is_safe_path_segment(app_id)) {
    return NULL;
  }

  g_autofree gchar *logical_path = g_strdup_printf("webapps/examples/%s/manifest.json", app_id);
  g_autofree gchar *packaged = packaged_resource_path_for_logical_path(logical_path);
  if (packaged != NULL && g_file_test(packaged, G_FILE_TEST_EXISTS)) {
    return g_steal_pointer(&packaged);
  }

  g_autofree gchar *root = repo_root();
  return g_build_filename(root, "webapps", "examples", app_id, "manifest.json", NULL);
}

static GHashTable *permissions_for_app(const gchar *app_id) {
  GHashTable *permissions = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
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

static GHashTable *resource_budget_for_app(const gchar *app_id) {
  GHashTable *budget = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    return budget;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    return budget;
  }
  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  JsonObject *raw_budget = json_object_get_object_member(manifest, "resourceBudget");
  if (raw_budget != NULL) {
    GList *members = json_object_get_members(raw_budget);
    for (GList *iter = members; iter != NULL; iter = iter->next) {
      const gchar *key = iter->data;
      JsonNode *value = json_object_get_member(raw_budget, key);
      if (value != NULL && JSON_NODE_HOLDS_VALUE(value)) {
        GType value_type = json_node_get_value_type(value);
        if (value_type == G_TYPE_INT64 || value_type == G_TYPE_INT || value_type == G_TYPE_DOUBLE) {
          guint limit = (guint)(value_type == G_TYPE_DOUBLE ? json_node_get_double(value) : json_node_get_int(value));
          g_hash_table_insert(budget, g_strdup(key), GUINT_TO_POINTER(limit));
        }
      }
    }
    g_list_free(members);
  }
  g_object_unref(parser);
  return budget;
}

static GPtrArray *network_policy_for_app(const gchar *app_id) {
  GPtrArray *rules = g_ptr_array_new_with_free_func(network_policy_rule_free);
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
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
      if (json_object_has_member(raw, "pathPrefix")) {
        rule->path_prefix = g_strdup(json_object_get_string_member(raw, "pathPrefix"));
      }
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
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
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

AppSandboxContext app_sandbox_context_for_app(const gchar *app_id, const gchar *mount_token) {
  return (AppSandboxContext){
      .app_id = g_strdup(app_id),
      .storage_prefix = g_strdup_printf("%s:", app_id),
      .approved_permissions = permissions_for_app(app_id),
      .network_policy = network_policy_for_app(app_id),
      .resource_budget = resource_budget_for_app(app_id),
      .deny_private_network = deny_private_network_for_app(app_id),
      .mount_token = g_strdup(mount_token),
  };
}
