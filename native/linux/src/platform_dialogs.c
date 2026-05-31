#include "platform_dialogs.h"

#include <gio/gio.h>
#include <string.h>

typedef struct {
  gint response;
  GMainLoop *loop;
} DialogRun;

static void dialog_response_cb(GtkNativeDialog *dialog, gint response, gpointer user_data) {
  (void)dialog;
  DialogRun *run = user_data;
  run->response = response;
  g_main_loop_quit(run->loop);
}

static gint run_native_dialog(GtkNativeDialog *dialog) {
  DialogRun run = {
      .response = GTK_RESPONSE_CANCEL,
      .loop = g_main_loop_new(NULL, FALSE),
  };
  g_signal_connect(dialog, "response", G_CALLBACK(dialog_response_cb), &run);
  gtk_native_dialog_show(dialog);
  g_main_loop_run(run.loop);
  g_main_loop_unref(run.loop);
  return run.response;
}

static gchar *runtime_session_id_for_request(const BridgeRequest *request) {
  const gchar *app_id = request->context.app_id != NULL ? request->context.app_id : "";
  const gchar *mount_token = request->context.mount_token != NULL && request->context.mount_token[0] != '\0'
      ? request->context.mount_token
      : "native";
  return g_strdup_printf("runtime_linux_%s_%s", app_id, mount_token);
}

static JsonNode *stored_dialog_mock(PlatformDialogs *dialogs, const BridgeRequest *request, const gchar *dialog_type) {
  if (dialogs == NULL || dialogs->db == NULL || request->context.app_id == NULL || request->context.app_id[0] == '\0') {
    return NULL;
  }

  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "SELECT response_json FROM dialog_mocks "
      "WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) "
      "ORDER BY created_at DESC LIMIT 1";
  if (sqlite3_prepare_v2(dialogs->db, sql, -1, &statement, NULL) != SQLITE_OK) {
    return NULL;
  }

  g_autofree gchar *session_id = runtime_session_id_for_request(request);
  sqlite3_bind_text(statement, 1, dialog_type, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 3, session_id, -1, SQLITE_TRANSIENT);

  JsonNode *mock = NULL;
  if (sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_text(statement, 0) != NULL) {
    const gchar *response_json = (const gchar *)sqlite3_column_text(statement, 0);
    JsonParser *parser = json_parser_new();
    if (json_parser_load_from_data(parser, response_json, -1, NULL)) {
      JsonNode *root = json_parser_get_root(parser);
      if (root != NULL) {
        mock = json_node_copy(root);
      }
    }
    g_object_unref(parser);
  }
  sqlite3_finalize(statement);
  return mock;
}

static gboolean params_bool(JsonObject *params, const gchar *name, gboolean fallback) {
  if (!json_object_has_member(params, name)) {
    return fallback;
  }
  JsonNode *node = json_object_get_member(params, name);
  return JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) == G_TYPE_BOOLEAN ? json_node_get_boolean(node) : fallback;
}

static gsize max_bytes_for_request(const BridgeRequest *request) {
  gint64 value = json_object_get_int_member_with_default(request->params, "maxBytes", 1024 * 1024);
  return value <= 0 ? 0 : (gsize)value;
}

static gchar *mime_for_file(const gchar *path, const gchar *contents, gsize length, const BridgeRequest *request) {
  if (json_object_has_member(request->params, "accept")) {
    JsonArray *accept = json_object_get_array_member(request->params, "accept");
    if (accept != NULL && json_array_get_length(accept) > 0) {
      const gchar *first = json_array_get_string_element(accept, 0);
      if (first != NULL && first[0] != '\0') {
        return g_strdup(first);
      }
    }
  }

  gboolean uncertain = FALSE;
  gchar *content_type = g_content_type_guess(path, (const guchar *)contents, length, &uncertain);
  gchar *mime = content_type == NULL ? NULL : g_content_type_get_mime_type(content_type);
  g_free(content_type);
  return mime == NULL ? g_strdup("text/plain") : mime;
}

static void add_accept_filters(GtkFileChooser *chooser, const BridgeRequest *request) {
  if (!json_object_has_member(request->params, "accept")) {
    return;
  }

  JsonArray *accept = json_object_get_array_member(request->params, "accept");
  if (accept == NULL || json_array_get_length(accept) == 0) {
    return;
  }

  GtkFileFilter *filter = gtk_file_filter_new();
  gtk_file_filter_set_name(filter, "Allowed files");
  for (guint index = 0; index < json_array_get_length(accept); ++index) {
    const gchar *mime = json_array_get_string_element(accept, index);
    if (mime != NULL && mime[0] != '\0') {
      gtk_file_filter_add_mime_type(filter, mime);
    }
  }
  gtk_file_chooser_add_filter(chooser, filter);
}

void platform_dialogs_init(PlatformDialogs *dialogs, GtkWindow *owner, sqlite3 *db) {
  if (dialogs == NULL) {
    return;
  }
  dialogs->owner = owner;
  dialogs->db = db;
}

JsonNode *platform_dialogs_open_file(PlatformDialogs *dialogs, const BridgeRequest *request) {
  JsonNode *mock = stored_dialog_mock(dialogs, request, "openFile");
  if (mock != NULL) {
    return bridge_success(request, mock);
  }

  GtkFileChooserNative *dialog = gtk_file_chooser_native_new(
      "Open file",
      dialogs == NULL ? NULL : dialogs->owner,
      GTK_FILE_CHOOSER_ACTION_OPEN,
      "Open",
      "Cancel");
  GtkFileChooser *chooser = GTK_FILE_CHOOSER(dialog);
  gtk_file_chooser_set_select_multiple(chooser, params_bool(request->params, "multiple", FALSE));
  add_accept_filters(chooser, request);

  gint response = run_native_dialog(GTK_NATIVE_DIALOG(dialog));
  if (response != GTK_RESPONSE_ACCEPT) {
    g_object_unref(dialog);
    return bridge_failure(request, "dialog_cancelled", "Open file was cancelled", NULL);
  }

  GFile *file = gtk_file_chooser_get_file(chooser);
  g_object_unref(dialog);
  if (file == NULL) {
    return bridge_failure(request, "storage_error", "Open file result was unavailable", NULL);
  }

  g_autofree gchar *path = g_file_get_path(file);
  g_object_unref(file);
  if (path == NULL) {
    return bridge_failure(request, "storage_error", "Open file path was unavailable", NULL);
  }

  gchar *contents = NULL;
  gsize length = 0;
  GError *error = NULL;
  if (!g_file_get_contents(path, &contents, &length, &error)) {
    g_clear_error(&error);
    return bridge_failure(request, "storage_error", "Selected file could not be read", NULL);
  }
  g_autofree gchar *owned_contents = contents;
  if (length > max_bytes_for_request(request)) {
    return bridge_failure(request, "quota_exceeded", "Selected file exceeds maxBytes", NULL);
  }

  g_autofree gchar *base_name = g_path_get_basename(path);
  g_autofree gchar *mime = mime_for_file(path, owned_contents, length, request);
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "files");
  json_builder_begin_array(builder);
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "name");
  json_builder_add_string_value(builder, base_name);
  json_builder_set_member_name(builder, "mime");
  json_builder_add_string_value(builder, mime);
  json_builder_set_member_name(builder, "size");
  json_builder_add_int_value(builder, (gint64)length);
  json_builder_set_member_name(builder, "text");
  json_builder_add_string_value(builder, owned_contents);
  json_builder_end_object(builder);
  json_builder_end_array(builder);
  json_builder_end_object(builder);
  JsonNode *result = json_builder_get_root(builder);
  g_object_unref(builder);
  return bridge_success(request, result);
}

JsonNode *platform_dialogs_save_file(PlatformDialogs *dialogs, const BridgeRequest *request) {
  JsonNode *mock = stored_dialog_mock(dialogs, request, "saveFile");
  if (mock != NULL) {
    return bridge_success(request, mock);
  }

  GtkFileChooserNative *dialog = gtk_file_chooser_native_new(
      "Save file",
      dialogs == NULL ? NULL : dialogs->owner,
      GTK_FILE_CHOOSER_ACTION_SAVE,
      "Save",
      "Cancel");
  GtkFileChooser *chooser = GTK_FILE_CHOOSER(dialog);
  const gchar *suggested = json_object_get_string_member_with_default(request->params, "suggestedName", "output.txt");
  gtk_file_chooser_set_current_name(chooser, suggested);

  gint response = run_native_dialog(GTK_NATIVE_DIALOG(dialog));
  if (response != GTK_RESPONSE_ACCEPT) {
    g_object_unref(dialog);
    return bridge_failure(request, "dialog_cancelled", "Save file was cancelled", NULL);
  }

  GFile *file = gtk_file_chooser_get_file(chooser);
  g_object_unref(dialog);
  if (file == NULL) {
    return bridge_failure(request, "storage_error", "Save file result was unavailable", NULL);
  }

  g_autofree gchar *path = g_file_get_path(file);
  g_object_unref(file);
  if (path == NULL) {
    return bridge_failure(request, "storage_error", "Save file path was unavailable", NULL);
  }

  const gchar *text = json_object_get_string_member_with_default(request->params, "text", "");
  GError *error = NULL;
  if (!g_file_set_contents(path, text, -1, &error)) {
    g_clear_error(&error);
    return bridge_failure(request, "storage_error", "Could not write selected file", NULL);
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  JsonNode *result = json_builder_get_root(builder);
  g_object_unref(builder);
  return bridge_success(request, result);
}
