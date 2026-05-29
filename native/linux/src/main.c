#include "platform_database.h"
#include "webkit_host.h"

#include <json-glib/json-glib.h>
#include <sqlite3.h>
#include <string.h>
#include <unistd.h>

static gboolean native_ai_debug_build_allows_dev_flags(void) {
#ifndef NDEBUG
  return TRUE;
#else
  return FALSE;
#endif
}

static gboolean native_ai_is_forbidden_dev_flag(const char *argument) {
  static const char *flags[] = {
      "--control-plane-port",
      "--allow-runtime-mismatch",
      "--allow-unsigned-dev",
  };
  for (gsize index = 0; index < G_N_ELEMENTS(flags); index++) {
    const char *flag = flags[index];
    gsize flag_length = strlen(flag);
    if (g_strcmp0(argument, flag) == 0) {
      return TRUE;
    }
    if (g_str_has_prefix(argument, flag) && argument[flag_length] == '=') {
      return TRUE;
    }
  }
  return FALSE;
}

static gchar *native_ai_database_path(void) {
  g_autofree gchar *data_dir = g_build_filename(g_get_user_data_dir(), "NativeAIWebappPlatform", NULL);
  g_mkdir_with_parents(data_dir, 0700);
  return g_build_filename(data_dir, "platform.sqlite", NULL);
}

static gchar *native_ai_json_for_flag(const char *flag) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "flag");
  json_builder_add_string_value(builder, flag);
  json_builder_end_object(builder);

  JsonNode *root = json_builder_get_root(builder);
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, root);
  gchar *text = json_generator_to_data(generator, NULL);
  json_node_unref(root);
  g_object_unref(generator);
  g_object_unref(builder);
  return text;
}

static gchar *native_ai_error_json_for_flag(const char *flag) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "code");
  json_builder_add_string_value(builder, "dev_only_flag");
  json_builder_set_member_name(builder, "message");
  json_builder_add_string_value(builder, "Production build rejects dev-only flag");
  json_builder_set_member_name(builder, "details");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "flag");
  json_builder_add_string_value(builder, flag);
  json_builder_end_object(builder);
  json_builder_end_object(builder);

  JsonNode *root = json_builder_get_root(builder);
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, root);
  gchar *text = json_generator_to_data(generator, NULL);
  json_node_unref(root);
  g_object_unref(generator);
  g_object_unref(builder);
  return text;
}

static gchar *native_ai_metadata_json_for_flag(const char *flag) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "reason");
  json_builder_add_string_value(builder, "dev_only_flag");
  json_builder_set_member_name(builder, "flag");
  json_builder_add_string_value(builder, flag);
  json_builder_end_object(builder);

  JsonNode *root = json_builder_get_root(builder);
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, root);
  gchar *text = json_generator_to_data(generator, NULL);
  json_node_unref(root);
  g_object_unref(generator);
  g_object_unref(builder);
  return text;
}

static void native_ai_bind_text(sqlite3_stmt *statement, int index, const gchar *value) {
  sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT);
}

static void native_ai_record_production_guard_audit(const char *flag) {
  g_autofree gchar *db_path = native_ai_database_path();
  sqlite3 *db = platform_database_open(db_path);
  if (db == NULL) {
    return;
  }

  gint64 now = g_get_real_time();
  g_autofree gchar *session_id = g_strdup_printf("linux-production-guard-%d-%" G_GINT64_FORMAT, getpid(), now);
  g_autofree gchar *command_id = g_strdup_printf("command-linux-production-guard-%d-%" G_GINT64_FORMAT, getpid(), now);
  g_autofree gchar *args_json = native_ai_json_for_flag(flag);
  g_autofree gchar *error_json = native_ai_error_json_for_flag(flag);
  g_autofree gchar *metadata_json = native_ai_metadata_json_for_flag(flag);

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT OR REPLACE INTO control_sessions "
          "(control_session_id, target, actor, started_at, ended_at, status, metadata_json) "
          "VALUES (?, 'linux', 'native-production-guard', datetime('now'), datetime('now'), 'failed', ?)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    native_ai_bind_text(statement, 1, session_id);
    native_ai_bind_text(statement, 2, metadata_json);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);

  statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT INTO control_commands "
          "(command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) "
          "VALUES (?, ?, 'native.production_guard', NULL, NULL, 'rejected', 'dev_only_flag', ?, NULL, ?, datetime('now'), 0)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    native_ai_bind_text(statement, 1, command_id);
    native_ai_bind_text(statement, 2, session_id);
    native_ai_bind_text(statement, 3, args_json);
    native_ai_bind_text(statement, 4, error_json);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
}

static gboolean native_ai_reject_dev_only_flags_if_needed(int argc, char **argv) {
  if (native_ai_debug_build_allows_dev_flags()) {
    return FALSE;
  }
  for (int index = 1; index < argc; index++) {
    if (native_ai_is_forbidden_dev_flag(argv[index])) {
      native_ai_record_production_guard_audit(argv[index]);
      g_printerr("fatal: production build rejects dev-only startup flag %s\n", argv[index]);
      return TRUE;
    }
  }
  return FALSE;
}

static void on_activate(GtkApplication *application, gpointer user_data) {
  (void)user_data;
  WebKitHost *host = webkit_host_new(application);
  webkit_host_present(host);
  g_object_set_data_full(G_OBJECT(application), "native-ai-webapp-host", host, (GDestroyNotify)webkit_host_free);
}

int main(int argc, char **argv) {
  if (native_ai_reject_dev_only_flags_if_needed(argc, argv)) {
    return 1;
  }

  GtkApplication *application = gtk_application_new("dev.nativeai.webappplatform", G_APPLICATION_DEFAULT_FLAGS);
  g_signal_connect(application, "activate", G_CALLBACK(on_activate), NULL);
  int status = g_application_run(G_APPLICATION(application), argc, argv);
  g_object_unref(application);
  return status;
}
