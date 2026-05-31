#include "dev_control_plane.h"

#include "app_sandbox.h"
#include "platform_database.h"

#include <errno.h>
#include <fcntl.h>
#include <glib/gstdio.h>
#include <sqlite3.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef NDEBUG
#include <gio/gio.h>
#include <json-glib/json-glib.h>
#include <libsoup/soup.h>

struct _DevControlPlane {
  SoupServer *server;
  WebBridge *bridge;
  gchar *database_path;
  gchar *control_session_id;
  gchar *token;
  gchar *token_hash;
  gchar *token_path;
  guint port;
  gboolean ready_announced;
};

static void bind_text(sqlite3_stmt *statement, int index, const gchar *value) {
  sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT);
}

static void bind_nullable_text(sqlite3_stmt *statement, int index, const gchar *value) {
  if (value == NULL || value[0] == '\0') {
    sqlite3_bind_null(statement, index);
    return;
  }
  sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT);
}

static gchar *now_iso(void) {
  GDateTime *now = g_date_time_new_now_utc();
  gchar *text = g_date_time_format_iso8601(now);
  g_date_time_unref(now);
  return text;
}

static gchar *make_id(const gchar *prefix) {
  return g_strdup_printf("%s-%d-%" G_GINT64_FORMAT "-%08x", prefix, getpid(), g_get_real_time(), g_random_int());
}

static gchar *make_snapshot_id(void) {
  return g_strdup_printf("snapshot_%d_%" G_GINT64_FORMAT "_%08x", getpid(), g_get_real_time(), g_random_int());
}

static gchar *json_escape(const gchar *text) {
  GString *out = g_string_new("");
  const gchar *safe = text == NULL ? "" : text;
  for (const gchar *cursor = safe; *cursor != '\0'; cursor++) {
    switch (*cursor) {
      case '"':
        g_string_append(out, "\\\"");
        break;
      case '\\':
        g_string_append(out, "\\\\");
        break;
      case '\n':
        g_string_append(out, "\\n");
        break;
      case '\r':
        g_string_append(out, "\\r");
        break;
      case '\t':
        g_string_append(out, "\\t");
        break;
      default:
        g_string_append_c(out, *cursor);
        break;
    }
  }
  return g_string_free(out, FALSE);
}

static gchar *json_node_to_text(JsonNode *node) {
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, node);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  return text;
}

static gboolean json_node_matches_subset(JsonNode *actual, JsonNode *expected) {
  if (actual == NULL || expected == NULL) {
    return actual == expected;
  }
  if (!JSON_NODE_HOLDS_OBJECT(expected)) {
    g_autofree gchar *actual_text = json_node_to_text(actual);
    g_autofree gchar *expected_text = json_node_to_text(expected);
    return g_strcmp0(actual_text, expected_text) == 0;
  }
  if (!JSON_NODE_HOLDS_OBJECT(actual)) {
    return FALSE;
  }

  JsonObject *actual_object = json_node_get_object(actual);
  JsonObject *expected_object = json_node_get_object(expected);
  GList *members = json_object_get_members(expected_object);
  gboolean matches = TRUE;
  for (GList *item = members; item != NULL; item = item->next) {
    const gchar *member = item->data;
    if (!json_object_has_member(actual_object, member) ||
        !json_node_matches_subset(json_object_get_member(actual_object, member), json_object_get_member(expected_object, member))) {
      matches = FALSE;
      break;
    }
  }
  g_list_free(members);
  return matches;
}

static gchar *json_builder_to_text(JsonBuilder *builder) {
  JsonNode *root = json_builder_get_root(builder);
  gchar *text = json_node_to_text(root);
  json_node_unref(root);
  return text;
}

static void json_builder_add_json_text_or_null(JsonBuilder *builder, const gchar *text) {
  if (text == NULL || text[0] == '\0') {
    json_builder_add_null_value(builder);
    return;
  }
  JsonParser *parser = json_parser_new();
  if (json_parser_load_from_data(parser, text, -1, NULL)) {
    JsonNode *root = json_parser_get_root(parser);
    if (root != NULL) {
      json_builder_add_value(builder, json_node_copy(root));
      g_object_unref(parser);
      return;
    }
  }
  g_object_unref(parser);
  json_builder_add_null_value(builder);
}

static gchar *control_ok_json(const gchar *result_json) {
  return g_strdup_printf("{\"ok\":true,\"result\":%s}", result_json == NULL ? "{}" : result_json);
}

static gchar *token_file_path(GError **error) {
  const gchar *override = g_getenv("PLATFORM_CONTROL_TOKEN_FILE");
  if (override != NULL && override[0] != '\0') {
    return g_strdup(override);
  }

  const gchar *runtime_dir = g_getenv("XDG_RUNTIME_DIR");
  if (runtime_dir == NULL || runtime_dir[0] == '\0') {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_INVAL, "Linux dev control token file requires XDG_RUNTIME_DIR or PLATFORM_CONTROL_TOKEN_FILE");
    return NULL;
  }
  return g_build_filename(runtime_dir, "native-ai-webapp", "control.token", NULL);
}

static gboolean read_exact_random(guint8 *bytes, gsize len) {
  int fd = g_open("/dev/urandom", O_RDONLY, 0);
  if (fd < 0) {
    return FALSE;
  }
  gsize offset = 0;
  while (offset < len) {
    ssize_t read_count = read(fd, bytes + offset, len - offset);
    if (read_count < 0) {
      if (errno == EINTR) {
        continue;
      }
      close(fd);
      return FALSE;
    }
    if (read_count == 0) {
      close(fd);
      return FALSE;
    }
    offset += (gsize)read_count;
  }
  close(fd);
  return TRUE;
}

static gchar *generate_control_token(GError **error) {
  guint8 bytes[32];
  if (!read_exact_random(bytes, sizeof(bytes))) {
    g_set_error_literal(error, G_FILE_ERROR, g_file_error_from_errno(errno), "Could not read random bytes for control token");
    return NULL;
  }

  gchar *encoded = g_base64_encode(bytes, sizeof(bytes));
  GString *token = g_string_new("");
  for (gchar *cursor = encoded; *cursor != '\0'; cursor++) {
    if (*cursor == '+') {
      g_string_append_c(token, '-');
    } else if (*cursor == '/') {
      g_string_append_c(token, '_');
    } else if (*cursor != '=') {
      g_string_append_c(token, *cursor);
    }
  }
  g_free(encoded);
  return g_string_free(token, FALSE);
}

static gboolean write_control_token_file(const gchar *path, const gchar *token, GError **error) {
  g_autofree gchar *parent = g_path_get_dirname(path);
  if (g_mkdir_with_parents(parent, 0700) != 0) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "Could not create control token directory: %s", g_strerror(errno));
    return FALSE;
  }

  int fd = g_open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
  if (fd < 0) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "Could not open control token file: %s", g_strerror(errno));
    return FALSE;
  }
  fchmod(fd, 0600);

  g_autofree gchar *line = g_strdup_printf("%s\n", token);
  gsize len = strlen(line);
  gsize offset = 0;
  while (offset < len) {
    ssize_t written = write(fd, line + offset, len - offset);
    if (written < 0) {
      if (errno == EINTR) {
        continue;
      }
      int saved_errno = errno;
      close(fd);
      g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(saved_errno), "Could not write control token file: %s", g_strerror(saved_errno));
      return FALSE;
    }
    offset += (gsize)written;
  }

  if (close(fd) != 0) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno), "Could not close control token file: %s", g_strerror(errno));
    return FALSE;
  }
  return TRUE;
}

static void insert_control_session(DevControlPlane *plane) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    return;
  }

  g_autofree gchar *escaped_token_path = json_escape(plane->token_path);
  g_autofree gchar *metadata = g_strdup_printf("{\"port\":%u,\"tokenPath\":\"%s\",\"kind\":\"listener\"}", plane->port, escaped_token_path);
  g_autofree gchar *started_at = now_iso();
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT OR REPLACE INTO control_sessions "
          "(control_session_id, target, actor, token_hash, started_at, status, metadata_json) "
          "VALUES (?, 'linux', 'codex', ?, ?, 'running', ?)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, plane->control_session_id);
    bind_text(statement, 2, plane->token_hash);
    bind_text(statement, 3, started_at);
    bind_text(statement, 4, metadata);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
}

static void finish_control_session(DevControlPlane *plane) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    return;
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "UPDATE control_sessions SET ended_at = ?, status = 'ended' WHERE control_session_id = ?",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    g_autofree gchar *ended_at = now_iso();
    bind_text(statement, 1, ended_at);
    bind_text(statement, 2, plane->control_session_id);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
}

static gchar *health_result_json(DevControlPlane *plane) {
  return g_strdup_printf(
      "{\"ok\":true,\"target\":\"linux\",\"status\":\"ok\",\"controlPlane\":{\"port\":%u,\"debug\":true}}",
      plane->port);
}

static gchar *error_json(const gchar *code, const gchar *message) {
  g_autofree gchar *escaped_code = json_escape(code);
  g_autofree gchar *escaped_message = json_escape(message);
  return g_strdup_printf(
      "{\"ok\":false,\"error\":{\"code\":\"%s\",\"message\":\"%s\",\"details\":{}}}",
      escaped_code,
      escaped_message);
}

static void audit_control_request(
    DevControlPlane *plane,
    const gchar *audit_session_id,
    const gchar *tool,
    const gchar *method,
    const gchar *path,
    const gchar *decision,
    const gchar *error_code,
    const gchar *result_json,
    const gchar *error_body,
    gint64 duration_ms) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    return;
  }

  g_autofree gchar *command_id = g_strdup_printf("linux-control-%d-%" G_GINT64_FORMAT "-%08x", getpid(), g_get_real_time(), g_random_int());
  g_autofree gchar *created_at = now_iso();
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT INTO control_commands "
          "(command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) "
          "VALUES (?, ?, ?, ?, ?, ?, ?, '{}', ?, ?, ?, ?)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, command_id);
    bind_text(statement, 2, audit_session_id == NULL ? plane->control_session_id : audit_session_id);
    bind_text(statement, 3, tool);
    bind_text(statement, 4, method);
    bind_text(statement, 5, path);
    bind_text(statement, 6, decision);
    if (error_code == NULL) {
      sqlite3_bind_null(statement, 7);
    } else {
      bind_text(statement, 7, error_code);
    }
    if (result_json == NULL) {
      sqlite3_bind_null(statement, 8);
    } else {
      bind_text(statement, 8, result_json);
    }
    if (error_body == NULL) {
      sqlite3_bind_null(statement, 9);
    } else {
      bind_text(statement, 9, error_body);
    }
    bind_text(statement, 10, created_at);
    sqlite3_bind_int64(statement, 11, duration_ms);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
}

static gboolean request_has_valid_token(DevControlPlane *plane, SoupServerMessage *message) {
  SoupMessageHeaders *headers = soup_server_message_get_request_headers(message);
  const gchar *token = soup_message_headers_get_one(headers, "X-Platform-Control-Token");
  return token != NULL && g_strcmp0(token, plane->token) == 0;
}

static void send_json(SoupServerMessage *message, guint status, const gchar *body) {
  soup_server_message_set_status(message, status, NULL);
  soup_server_message_set_response(message, "application/json", SOUP_MEMORY_COPY, body, strlen(body));
}

static gboolean authorize_request(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started) {
  if (request_has_valid_token(plane, message)) {
    return TRUE;
  }
  g_autofree gchar *body = error_json("control_auth_required", "Missing or invalid control token");
  send_json(message, SOUP_STATUS_UNAUTHORIZED, body);
  audit_control_request(plane, NULL, "control.auth", method, path, "rejected", "control_auth_required", NULL, body, (g_get_real_time() - started) / 1000);
  return FALSE;
}

static gchar *request_body_text(SoupServerMessage *message) {
  SoupMessageBody *body = soup_server_message_get_request_body(message);
  if (body == NULL || body->data == NULL || body->length <= 0) {
    return g_strdup("{}");
  }
  return g_strndup(body->data, (gsize)body->length);
}

static JsonObject *parse_request_object(SoupServerMessage *message, JsonParser **parser_out, GError **error) {
  g_autofree gchar *body = request_body_text(message);
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, body, -1, error)) {
    g_object_unref(parser);
    return NULL;
  }
  JsonNode *root = json_parser_get_root(parser);
  if (root == NULL || !JSON_NODE_HOLDS_OBJECT(root)) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Control request body must be a JSON object");
    g_object_unref(parser);
    return NULL;
  }
  *parser_out = parser;
  return json_node_get_object(root);
}

static const gchar *object_string(JsonObject *object, const gchar *member, const gchar *fallback) {
  if (object != NULL && json_object_has_member(object, member)) {
    JsonNode *node = json_object_get_member(object, member);
    if (node != NULL && JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) == G_TYPE_STRING) {
      return json_node_get_string(node);
    }
  }
  return fallback;
}

static const gchar *object_string_any(JsonObject *object, const gchar *first, const gchar *second, const gchar *third, const gchar *fallback) {
  const gchar *members[] = {first, second, third};
  for (gsize index = 0; index < G_N_ELEMENTS(members); index++) {
    const gchar *member = members[index];
    const gchar *value = member == NULL ? NULL : object_string(object, member, NULL);
    if (value != NULL) {
      return value;
    }
  }
  return fallback;
}

static gint64 object_int_any(JsonObject *object, const gchar *first, const gchar *second, const gchar *third, gint64 fallback) {
  const gchar *members[] = {first, second, third};
  for (gsize index = 0; index < G_N_ELEMENTS(members); index++) {
    const gchar *member = members[index];
    if (object == NULL || member == NULL || !json_object_has_member(object, member)) {
      continue;
    }
    JsonNode *node = json_object_get_member(object, member);
    if (node == NULL || !JSON_NODE_HOLDS_VALUE(node)) {
      continue;
    }
    GType value_type = json_node_get_value_type(node);
    if (value_type == G_TYPE_INT64 || value_type == G_TYPE_INT || value_type == G_TYPE_LONG || value_type == G_TYPE_UINT || value_type == G_TYPE_UINT64) {
      return json_node_get_int(node);
    }
    if (value_type == G_TYPE_BOOLEAN) {
      return json_node_get_boolean(node) ? 1 : 0;
    }
    if (value_type == G_TYPE_DOUBLE) {
      return (gint64)json_node_get_double(node);
    }
  }
  return fallback;
}

static gboolean object_boolean_true(JsonObject *object, const gchar *member) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return FALSE;
  }
  JsonNode *node = json_object_get_member(object, member);
  return node != NULL && JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) == G_TYPE_BOOLEAN && json_node_get_boolean(node);
}

static JsonObject *object_object(JsonObject *object, const gchar *member) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return NULL;
  }
  JsonNode *node = json_object_get_member(object, member);
  return node != NULL && JSON_NODE_HOLDS_OBJECT(node) ? json_node_get_object(node) : NULL;
}

static gboolean valid_generated_app_id(const gchar *app_id) {
  return app_id != NULL && g_regex_match_simple("^[a-z][a-z0-9-]{2,63}$", app_id, 0, 0);
}

static gchar *object_member_json(JsonObject *object, const gchar *member, const gchar *fallback_json) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return g_strdup(fallback_json);
  }
  JsonNode *node = json_object_get_member(object, member);
  if (node == NULL) {
    return g_strdup(fallback_json);
  }
  return json_node_to_text(node);
}

static gchar *object_json_text_any(JsonObject *object, const gchar *first_text, const gchar *second_text, const gchar *object_member, const gchar *fallback_json) {
  const gchar *text = object_string_any(object, first_text, second_text, NULL, NULL);
  if (text != NULL) {
    return g_strdup(text);
  }
  if (object != NULL && object_member != NULL && json_object_has_member(object, object_member)) {
    JsonNode *node = json_object_get_member(object, object_member);
    if (node != NULL && !JSON_NODE_HOLDS_NULL(node)) {
      return json_node_to_text(node);
    }
  }
  return fallback_json == NULL ? NULL : g_strdup(fallback_json);
}

static JsonArray *object_array(JsonObject *object, const gchar *member) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return NULL;
  }
  JsonNode *node = json_object_get_member(object, member);
  return node != NULL && JSON_NODE_HOLDS_ARRAY(node) ? json_node_get_array(node) : NULL;
}

static gboolean json_array_object_at(JsonArray *array, guint index, JsonObject **object_out) {
  JsonNode *node = array == NULL ? NULL : json_array_get_element(array, index);
  if (node == NULL || !JSON_NODE_HOLDS_OBJECT(node)) {
    return FALSE;
  }
  *object_out = json_node_get_object(node);
  return TRUE;
}

static const gchar *object_nonempty_string(JsonObject *object, const gchar *member) {
  const gchar *value = object_string(object, member, NULL);
  return value == NULL || value[0] == '\0' ? NULL : value;
}

static gchar *upper_ascii(const gchar *text) {
  return g_ascii_strup(text == NULL || text[0] == '\0' ? "GET" : text, -1);
}

static const gchar *network_mock_url_pattern(JsonObject *args) {
  const gchar *direct = object_nonempty_string(args, "urlPattern");
  if (direct != NULL) {
    return direct;
  }
  JsonObject *match = object_object(args, "match");
  const gchar *match_pattern = object_nonempty_string(match, "urlPattern");
  if (match_pattern != NULL) {
    return match_pattern;
  }
  return object_nonempty_string(match, "url");
}

static gchar *network_mock_method(JsonObject *args) {
  const gchar *method = object_nonempty_string(args, "method");
  if (method != NULL) {
    return upper_ascii(method);
  }
  JsonObject *match = object_object(args, "match");
  return upper_ascii(object_nonempty_string(match, "method"));
}

static const gchar *dialog_mock_type(JsonObject *args) {
  const gchar *raw = object_nonempty_string(args, "dialogType");
  if (raw == NULL) {
    raw = object_nonempty_string(args, "method");
  }
  if (raw == NULL) {
    return NULL;
  }
  if (g_str_has_prefix(raw, "dialog.")) {
    raw += strlen("dialog.");
  }
  return g_strcmp0(raw, "openFile") == 0 || g_strcmp0(raw, "saveFile") == 0 ? raw : NULL;
}

static gchar *dialog_mock_response_json(JsonObject *args) {
  if (json_object_has_member(args, "response") && !json_object_get_null_member(args, "response")) {
    return json_node_to_text(json_object_get_member(args, "response"));
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "files");
  if (json_object_has_member(args, "files")) {
    json_builder_add_value(builder, json_node_copy(json_object_get_member(args, "files")));
  } else {
    json_builder_begin_array(builder);
    json_builder_end_array(builder);
  }
  json_builder_set_member_name(builder, "selectedPath");
  if (json_object_has_member(args, "selectedPath")) {
    json_builder_add_value(builder, json_node_copy(json_object_get_member(args, "selectedPath")));
  } else {
    json_builder_add_null_value(builder);
  }
  json_builder_set_member_name(builder, "cancelled");
  if (json_object_has_member(args, "cancelled")) {
    JsonNode *cancelled = json_object_get_member(args, "cancelled");
    if (cancelled != NULL && JSON_NODE_HOLDS_VALUE(cancelled) && json_node_get_value_type(cancelled) == G_TYPE_BOOLEAN) {
      json_builder_add_boolean_value(builder, json_node_get_boolean(cancelled));
    } else {
      json_builder_add_boolean_value(builder, FALSE);
    }
  } else {
    json_builder_add_boolean_value(builder, FALSE);
  }
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *runtime_network_mock_set_json(DevControlPlane *plane, JsonObject *args, GError **error) {
  const gchar *url_pattern = network_mock_url_pattern(args);
  if (url_pattern == NULL || !json_object_has_member(args, "response") || json_object_get_null_member(args, "response")) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.network_mock_set requires urlPattern or match.url and response");
    return NULL;
  }

  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  const gchar *app_id = object_nonempty_string(args, "appId");
  const gchar *session_id = object_nonempty_string(args, "sessionId");
  g_autofree gchar *method_name = network_mock_method(args);
  g_autofree gchar *response_json = json_node_to_text(json_object_get_member(args, "response"));
  g_autofree gchar *mock_id = make_id("netmock");
  g_autofree gchar *created_at = now_iso();

  sqlite3_stmt *statement = NULL;
  gboolean ok = sqlite3_prepare_v2(
                   db,
                   "INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at) "
                   "VALUES (?, ?, ?, ?, ?, ?, 1, ?)",
                   -1,
                   &statement,
                   NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, mock_id);
    bind_nullable_text(statement, 2, session_id);
    bind_nullable_text(statement, 3, app_id);
    bind_text(statement, 4, method_name);
    bind_text(statement, 5, url_pattern);
    bind_text(statement, 6, response_json);
    bind_text(statement, 7, created_at);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
  if (!ok) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Network mock could not be registered");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "mockId");
  json_builder_add_string_value(builder, mock_id);
  json_builder_set_member_name(builder, "sessionId");
  session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, session_id);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, method_name);
  json_builder_set_member_name(builder, "urlPattern");
  json_builder_add_string_value(builder, url_pattern);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gint64 delete_mock_rows(sqlite3 *db, const gchar *sql, const gchar *first, const gchar *second, gboolean *ok) {
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    *ok = FALSE;
    return 0;
  }
  if (first != NULL && first[0] != '\0') {
    bind_text(statement, 1, first);
  }
  if (second != NULL && second[0] != '\0') {
    bind_text(statement, 2, second);
  }
  if (sqlite3_step(statement) != SQLITE_DONE) {
    *ok = FALSE;
    sqlite3_finalize(statement);
    return 0;
  }
  gint64 changes = sqlite3_changes(db);
  sqlite3_finalize(statement);
  return changes;
}

static gchar *runtime_network_mock_reset_json(DevControlPlane *plane, JsonObject *args, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  const gchar *app_id = object_nonempty_string(args, "appId");
  const gchar *session_id = object_nonempty_string(args, "sessionId");
  gboolean ok = TRUE;
  gint64 cleared = 0;
  if (session_id != NULL && app_id != NULL) {
    cleared = delete_mock_rows(db, "DELETE FROM network_mocks WHERE session_id = ? AND app_id = ?", session_id, app_id, &ok);
  } else if (session_id != NULL) {
    cleared = delete_mock_rows(db, "DELETE FROM network_mocks WHERE session_id = ?", session_id, NULL, &ok);
  } else if (app_id != NULL) {
    cleared = delete_mock_rows(db, "DELETE FROM network_mocks WHERE app_id = ?", app_id, NULL, &ok);
  } else {
    cleared = delete_mock_rows(db, "DELETE FROM network_mocks", NULL, NULL, &ok);
  }
  platform_database_close(db);
  if (!ok) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Network mocks could not be reset");
    return NULL;
  }
  return g_strdup_printf("{\"ok\":true,\"cleared\":%" G_GINT64_FORMAT "}", cleared);
}

static gchar *runtime_dialog_mock_set_json(DevControlPlane *plane, JsonObject *args, GError **error) {
  const gchar *dialog_type = dialog_mock_type(args);
  if (dialog_type == NULL) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.dialog_mock_set requires dialogType or method");
    return NULL;
  }

  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  const gchar *app_id = object_nonempty_string(args, "appId");
  const gchar *session_id = object_nonempty_string(args, "sessionId");
  g_autofree gchar *response_json = dialog_mock_response_json(args);
  g_autofree gchar *mock_id = make_id("dialogmock");
  g_autofree gchar *created_at = now_iso();
  sqlite3_stmt *statement = NULL;
  gboolean ok = sqlite3_prepare_v2(
                   db,
                   "INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at) "
                   "VALUES (?, ?, ?, ?, ?, 1, ?)",
                   -1,
                   &statement,
                   NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, mock_id);
    bind_nullable_text(statement, 2, session_id);
    bind_nullable_text(statement, 3, app_id);
    bind_text(statement, 4, dialog_type);
    bind_text(statement, 5, response_json);
    bind_text(statement, 6, created_at);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
  if (!ok) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Dialog mock could not be registered");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "mockId");
  json_builder_add_string_value(builder, mock_id);
  json_builder_set_member_name(builder, "sessionId");
  session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, session_id);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "dialogType");
  json_builder_add_string_value(builder, dialog_type);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *bridge_call_request_json(const gchar *request_id, const gchar *bridge_method, JsonNode *params_node) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "id");
  json_builder_add_string_value(builder, request_id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, bridge_method);
  json_builder_set_member_name(builder, "params");
  if (params_node == NULL) {
    json_builder_begin_object(builder);
    json_builder_end_object(builder);
  } else {
    json_builder_add_value(builder, json_node_copy(params_node));
  }
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *core_step_request_json(const gchar *request_id, JsonNode *event_node) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "id");
  json_builder_add_string_value(builder, request_id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, "core.step");
  json_builder_set_member_name(builder, "params");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "event");
  json_builder_add_value(builder, json_node_copy(event_node));
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *runtime_capabilities_json(DevControlPlane *plane, const gchar *app_id) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "runtimeVersion");
  json_builder_add_string_value(builder, "0.1.0");
  json_builder_set_member_name(builder, "platform");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, "linux-native");
  json_builder_set_member_name(builder, "appId");
  if (app_id == NULL) {
    json_builder_add_null_value(builder);
  } else {
    json_builder_add_string_value(builder, app_id);
  }
  json_builder_set_member_name(builder, "controlPlane");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "port");
  json_builder_add_int_value(builder, plane->port);
  json_builder_set_member_name(builder, "debug");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "routes");
  json_builder_begin_array(builder);
  const gchar *routes[] = {"GET /health", "POST /sessions", "DELETE /sessions/:id", "GET /sessions/:id/snapshot", "GET /sessions/:id/events", "GET /sessions/:id/capabilities", "POST /sessions/:id/command"};
  for (gsize index = 0; index < G_N_ELEMENTS(routes); index++) {
    json_builder_add_string_value(builder, routes[index]);
  }
  json_builder_end_array(builder);
  json_builder_end_object(builder);
  json_builder_set_member_name(builder, "features");
  json_builder_begin_object(builder);
  const gchar *features[] = {"storage.read", "storage.write", "storage.get", "storage.set", "storage.remove", "storage.list", "notification.toast", "network.request", "runtime.capabilities", "app.log", "core.step"};
  for (gsize index = 0; index < G_N_ELEMENTS(features); index++) {
    json_builder_set_member_name(builder, features[index]);
    json_builder_add_boolean_value(builder, TRUE);
  }
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *platform_list_targets_json(DevControlPlane *plane) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "targets");
  json_builder_begin_array(builder);
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "id");
  json_builder_add_string_value(builder, "linux-native");
  json_builder_set_member_name(builder, "platform");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "status");
  json_builder_add_string_value(builder, "available");
  json_builder_set_member_name(builder, "runtimeVersion");
  json_builder_add_string_value(builder, "0.1.0");
  json_builder_set_member_name(builder, "controlPlane");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "port");
  json_builder_add_int_value(builder, plane->port);
  json_builder_set_member_name(builder, "debug");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  json_builder_end_array(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static void json_builder_add_nullable_sql_text(JsonBuilder *builder, sqlite3_stmt *statement, int column) {
  const unsigned char *text = sqlite3_column_text(statement, column);
  if (text == NULL) {
    json_builder_add_null_value(builder);
  } else {
    json_builder_add_string_value(builder, (const gchar *)text);
  }
}

static void json_builder_add_manifest_summary_member(JsonBuilder *builder, const gchar *member, const gchar *app_id, const gchar *fallback) {
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    if (fallback == NULL) {
      json_builder_add_null_value(builder);
    } else {
      json_builder_add_string_value(builder, fallback);
    }
    return;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    if (fallback == NULL) {
      json_builder_add_null_value(builder);
    } else {
      json_builder_add_string_value(builder, fallback);
    }
    return;
  }

  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  const gchar *value = object_string(manifest, member, fallback);
  if (value == NULL) {
    json_builder_add_null_value(builder);
  } else {
    json_builder_add_string_value(builder, value);
  }
  g_object_unref(parser);
}

static gint64 manifest_data_version(const gchar *app_id) {
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  g_autofree gchar *contents = NULL;
  if (manifest_path == NULL || !g_file_get_contents(manifest_path, &contents, NULL, NULL)) {
    return 1;
  }

  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, contents, -1, NULL)) {
    g_object_unref(parser);
    return 1;
  }
  JsonObject *manifest = json_node_get_object(json_parser_get_root(parser));
  gint64 version = json_object_get_int_member_with_default(manifest, "dataVersion", 1);
  g_object_unref(parser);
  return version;
}

static void append_bundled_webapp(JsonBuilder *builder, const gchar *app_id, GHashTable *installed_ids) {
  if (g_hash_table_contains(installed_ids, app_id)) {
    return;
  }

  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "name");
  json_builder_add_manifest_summary_member(builder, "name", app_id, app_id);
  json_builder_set_member_name(builder, "version");
  json_builder_add_manifest_summary_member(builder, "version", app_id, NULL);
  json_builder_set_member_name(builder, "description");
  json_builder_add_manifest_summary_member(builder, "description", app_id, NULL);
  json_builder_set_member_name(builder, "status");
  json_builder_add_string_value(builder, "bundled");
  json_builder_set_member_name(builder, "dataVersion");
  json_builder_add_int_value(builder, manifest_data_version(app_id));
  json_builder_set_member_name(builder, "bundled");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "installed");
  json_builder_add_boolean_value(builder, FALSE);
  json_builder_end_object(builder);
}

static gchar *platform_list_webapps_json(DevControlPlane *plane, gboolean include_uninstalled, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  GHashTable *installed_ids = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "apps");
  json_builder_begin_array(builder);

  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version, "
      "a.created_at, a.updated_at, v.runtime_version, v.trust_level "
      "FROM apps a LEFT JOIN app_versions v ON v.install_id = a.active_install_id "
      "WHERE (? = 1 OR a.status <> 'uninstalled') ORDER BY a.id";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not list webapps: %s", sqlite3_errmsg(db));
    sqlite3_finalize(statement);
    g_hash_table_destroy(installed_ids);
    g_object_unref(builder);
    platform_database_close(db);
    return NULL;
  }

  sqlite3_bind_int(statement, 1, include_uninstalled ? 1 : 0);
  while (sqlite3_step(statement) == SQLITE_ROW) {
    const gchar *app_id = (const gchar *)sqlite3_column_text(statement, 0);
    if (app_id == NULL) {
      continue;
    }
    g_hash_table_add(installed_ids, g_strdup(app_id));
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "appId");
    json_builder_add_string_value(builder, app_id);
    json_builder_set_member_name(builder, "name");
    json_builder_add_nullable_sql_text(builder, statement, 1);
    json_builder_set_member_name(builder, "status");
    json_builder_add_nullable_sql_text(builder, statement, 2);
    json_builder_set_member_name(builder, "activeInstallId");
    json_builder_add_nullable_sql_text(builder, statement, 3);
    json_builder_set_member_name(builder, "activeVersion");
    json_builder_add_nullable_sql_text(builder, statement, 4);
    json_builder_set_member_name(builder, "dataVersion");
    json_builder_add_int_value(builder, sqlite3_column_int64(statement, 5));
    json_builder_set_member_name(builder, "runtimeVersion");
    json_builder_add_nullable_sql_text(builder, statement, 8);
    json_builder_set_member_name(builder, "trustLevel");
    json_builder_add_nullable_sql_text(builder, statement, 9);
    json_builder_set_member_name(builder, "createdAt");
    json_builder_add_nullable_sql_text(builder, statement, 6);
    json_builder_set_member_name(builder, "updatedAt");
    json_builder_add_nullable_sql_text(builder, statement, 7);
    json_builder_set_member_name(builder, "bundled");
    json_builder_add_boolean_value(builder, FALSE);
    json_builder_set_member_name(builder, "installed");
    json_builder_add_boolean_value(builder, TRUE);
    json_builder_end_object(builder);
  }
  sqlite3_finalize(statement);

  const gchar *bundled_ids[] = {"notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab"};
  for (gsize index = 0; index < G_N_ELEMENTS(bundled_ids); index++) {
    append_bundled_webapp(builder, bundled_ids[index], installed_ids);
  }

  json_builder_end_array(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_hash_table_destroy(installed_ids);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static gchar *active_install_id(sqlite3 *db, const gchar *app_id) {
  if (app_id == NULL || app_id[0] == '\0') {
    return NULL;
  }
  sqlite3_stmt *statement = NULL;
  gchar *install_id = NULL;
  if (sqlite3_prepare_v2(db, "SELECT active_install_id FROM apps WHERE id = ?", -1, &statement, NULL) == SQLITE_OK) {
    bind_text(statement, 1, app_id);
    if (sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_text(statement, 0) != NULL) {
      install_id = g_strdup((const gchar *)sqlite3_column_text(statement, 0));
    }
  }
  sqlite3_finalize(statement);
  return install_id;
}

static gboolean active_app_snapshot_metadata(sqlite3 *db, const gchar *app_id, gchar **install_id, gchar **active_version, gint64 *data_version) {
  *install_id = NULL;
  *active_version = NULL;
  *data_version = manifest_data_version(app_id);
  if (app_id == NULL || app_id[0] == '\0') {
    return TRUE;
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, "SELECT active_install_id, active_version, data_version FROM apps WHERE id = ?", -1, &statement, NULL) != SQLITE_OK) {
    sqlite3_finalize(statement);
    return FALSE;
  }
  bind_text(statement, 1, app_id);
  if (sqlite3_step(statement) == SQLITE_ROW) {
    if (sqlite3_column_text(statement, 0) != NULL) {
      *install_id = g_strdup((const gchar *)sqlite3_column_text(statement, 0));
    }
    if (sqlite3_column_text(statement, 1) != NULL) {
      *active_version = g_strdup((const gchar *)sqlite3_column_text(statement, 1));
    }
    *data_version = sqlite3_column_int64(statement, 2);
  }
  sqlite3_finalize(statement);
  return TRUE;
}

static gchar *create_control_session(DevControlPlane *plane, JsonObject *body, GError **error) {
  const gchar *app_id = object_string(body, "appId", NULL);
  const gchar *actor = object_string(body, "actor", "codex");
  const gchar *target = object_string(body, "target", "linux");
  g_autofree gchar *metadata_json = object_member_json(body, "metadata", "{}");
  g_autofree gchar *control_session_id = make_id("control");
  g_autofree gchar *runtime_session_id = app_id == NULL ? NULL : make_id("session");
  g_autofree gchar *started_at = now_iso();

  if (app_id != NULL && !valid_generated_app_id(app_id)) {
    g_set_error(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Control session appId is not a valid generated app id");
    return NULL;
  }

  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *install_id = active_install_id(db, app_id);
  g_autofree gchar *capabilities = runtime_capabilities_json(plane, app_id);
  g_autofree gchar *resource_usage = app_id == NULL
      ? g_strdup("{\"appId\":null,\"bridgeCalls\":0,\"coreEvents\":0}")
      : g_strdup_printf("{\"appId\":\"%s\",\"bridgeCalls\":0,\"coreEvents\":0}", app_id);
  g_autofree gchar *runtime_metadata = app_id == NULL
      ? NULL
      : g_strdup_printf("{\"controlSessionId\":\"%s\",\"source\":\"linux-dev-control\"}", control_session_id);

  char *sql_error = NULL;
  if (sqlite3_exec(db, "BEGIN IMMEDIATE", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not begin control session transaction: %s", sql_error == NULL ? sqlite3_errmsg(db) : sql_error);
    sqlite3_free(sql_error);
    platform_database_close(db);
    return NULL;
  }

  gboolean ok = TRUE;
  if (runtime_session_id != NULL) {
    sqlite3_stmt *runtime = NULL;
    ok = sqlite3_prepare_v2(
             db,
             "INSERT INTO runtime_sessions "
             "(session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) "
             "VALUES (?, 'linux', 'linux', '0.1.0', ?, ?, ?, 'running', ?, ?, ?)",
             -1,
             &runtime,
             NULL) == SQLITE_OK;
    if (ok) {
      bind_text(runtime, 1, runtime_session_id);
      bind_text(runtime, 2, app_id);
      if (install_id == NULL) {
        sqlite3_bind_null(runtime, 3);
      } else {
        bind_text(runtime, 3, install_id);
      }
      bind_text(runtime, 4, started_at);
      bind_text(runtime, 5, capabilities);
      bind_text(runtime, 6, resource_usage);
      bind_text(runtime, 7, runtime_metadata);
      ok = sqlite3_step(runtime) == SQLITE_DONE;
    }
    sqlite3_finalize(runtime);
  }

  sqlite3_stmt *control = NULL;
  ok = ok &&
       sqlite3_prepare_v2(
           db,
           "INSERT INTO control_sessions "
           "(control_session_id, target, runtime_session_id, actor, token_hash, started_at, status, metadata_json) "
           "VALUES (?, ?, ?, ?, ?, ?, 'running', ?)",
           -1,
           &control,
           NULL) == SQLITE_OK;
  if (ok) {
    bind_text(control, 1, control_session_id);
    bind_text(control, 2, target);
    if (runtime_session_id == NULL) {
      sqlite3_bind_null(control, 3);
    } else {
      bind_text(control, 3, runtime_session_id);
    }
    bind_text(control, 4, actor);
    bind_text(control, 5, plane->token_hash);
    bind_text(control, 6, started_at);
    bind_text(control, 7, metadata_json);
    ok = sqlite3_step(control) == SQLITE_DONE;
  }
  sqlite3_finalize(control);

  if (!ok) {
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not create control session: %s", sqlite3_errmsg(db));
    platform_database_close(db);
    return NULL;
  }
  sqlite3_exec(db, "COMMIT", NULL, NULL, NULL);
  platform_database_close(db);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "controlSessionId");
  json_builder_add_string_value(builder, control_session_id);
  json_builder_set_member_name(builder, "runtimeSessionId");
  runtime_session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, runtime_session_id);
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, target);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "status");
  json_builder_add_string_value(builder, "running");
  json_builder_set_member_name(builder, "startedAt");
  json_builder_add_string_value(builder, started_at);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

typedef struct {
  gchar *control_session_id;
  gchar *runtime_session_id;
  gchar *target;
  gchar *app_id;
  gchar *status;
  gchar *started_at;
  gchar *ended_at;
} ControlSessionRecord;

static void control_session_record_clear(ControlSessionRecord *record) {
  g_free(record->control_session_id);
  g_free(record->runtime_session_id);
  g_free(record->target);
  g_free(record->app_id);
  g_free(record->status);
  g_free(record->started_at);
  g_free(record->ended_at);
}

static gboolean load_control_session(sqlite3 *db, const gchar *control_session_id, ControlSessionRecord *record) {
  sqlite3_stmt *statement = NULL;
  gboolean found = FALSE;
  if (sqlite3_prepare_v2(
          db,
          "SELECT c.control_session_id, c.runtime_session_id, c.target, c.status, c.started_at, c.ended_at, r.active_app_id "
          "FROM control_sessions c LEFT JOIN runtime_sessions r ON r.session_id = c.runtime_session_id "
          "WHERE c.control_session_id = ?",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, control_session_id);
    if (sqlite3_step(statement) == SQLITE_ROW) {
      record->control_session_id = g_strdup((const gchar *)sqlite3_column_text(statement, 0));
      record->runtime_session_id = sqlite3_column_text(statement, 1) == NULL ? NULL : g_strdup((const gchar *)sqlite3_column_text(statement, 1));
      record->target = g_strdup((const gchar *)sqlite3_column_text(statement, 2));
      record->status = g_strdup((const gchar *)sqlite3_column_text(statement, 3));
      record->started_at = g_strdup((const gchar *)sqlite3_column_text(statement, 4));
      record->ended_at = sqlite3_column_text(statement, 5) == NULL ? NULL : g_strdup((const gchar *)sqlite3_column_text(statement, 5));
      record->app_id = sqlite3_column_text(statement, 6) == NULL ? NULL : g_strdup((const gchar *)sqlite3_column_text(statement, 6));
      found = TRUE;
    }
  }
  sqlite3_finalize(statement);
  return found;
}

static gchar *end_control_session(DevControlPlane *plane, const gchar *control_session_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  ControlSessionRecord record = {0};
  if (!load_control_session(db, control_session_id, &record)) {
    platform_database_close(db);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_NOENT, "Control session not found: %s", control_session_id);
    return NULL;
  }

  g_autofree gchar *ended_at = now_iso();
  sqlite3_stmt *statement = NULL;
  gboolean ok = sqlite3_prepare_v2(db, "UPDATE control_sessions SET status = 'ended', ended_at = ? WHERE control_session_id = ?", -1, &statement, NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, ended_at);
    bind_text(statement, 2, control_session_id);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  if (record.runtime_session_id != NULL) {
    statement = NULL;
    if (sqlite3_prepare_v2(db, "UPDATE runtime_sessions SET status = 'ended', ended_at = ? WHERE session_id = ?", -1, &statement, NULL) == SQLITE_OK) {
      bind_text(statement, 1, ended_at);
      bind_text(statement, 2, record.runtime_session_id);
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
  }
  platform_database_close(db);
  if (!ok) {
    control_session_record_clear(&record);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not end control session: %s", control_session_id);
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "controlSessionId");
  json_builder_add_string_value(builder, control_session_id);
  json_builder_set_member_name(builder, "runtimeSessionId");
  record.runtime_session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.runtime_session_id);
  json_builder_set_member_name(builder, "status");
  json_builder_add_string_value(builder, "ended");
  json_builder_set_member_name(builder, "endedAt");
  json_builder_add_string_value(builder, ended_at);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  control_session_record_clear(&record);
  return text;
}

static gint64 count_table_for_app(sqlite3 *db, const gchar *table, const gchar *app_id) {
  const gchar *sql = NULL;
  if (g_strcmp0(table, "bridge_calls") == 0) {
    sql = app_id == NULL ? "SELECT COUNT(*) FROM bridge_calls" : "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?";
  } else if (g_strcmp0(table, "core_events") == 0) {
    sql = app_id == NULL ? "SELECT COUNT(*) FROM core_events" : "SELECT COUNT(*) FROM core_events WHERE app_id = ?";
  } else if (g_strcmp0(table, "app_storage") == 0) {
    sql = app_id == NULL ? "SELECT COUNT(*) FROM app_storage" : "SELECT COUNT(*) FROM app_storage WHERE app_id = ?";
  } else {
    return 0;
  }
  sqlite3_stmt *statement = NULL;
  gint64 count = 0;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    if (sqlite3_step(statement) == SQLITE_ROW) {
      count = sqlite3_column_int64(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return count;
}

static gint64 scalar_int_query(sqlite3 *db, const gchar *sql, const gchar *app_id, const gchar *method) {
  sqlite3_stmt *statement = NULL;
  gint64 value = 0;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    if (method != NULL) {
      bind_text(statement, 2, method);
    }
    if (sqlite3_step(statement) == SQLITE_ROW) {
      value = sqlite3_column_int64(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return value;
}

static gchar *runtime_resource_usage_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  if (app_id == NULL || app_id[0] == '\0') {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.resource_usage requires appId");
    return NULL;
  }
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  gint64 storage_bytes = scalar_int_query(
      db,
      "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?",
      app_id,
      NULL);
  gint64 bridge_calls = count_table_for_app(db, "bridge_calls", app_id);
  gint64 core_events = count_table_for_app(db, "core_events", app_id);
  gint64 network_requests = scalar_int_query(
      db,
      "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND created_at >= datetime('now', '-60 seconds')",
      app_id,
      "network.request");
  gint64 log_lines = scalar_int_query(
      db,
      "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND created_at >= datetime('now', '-60 seconds')",
      app_id,
      "app.log");
  platform_database_close(db);

  g_autofree gchar *measured_at = now_iso();
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "storageBytes");
  json_builder_add_int_value(builder, storage_bytes);
  json_builder_set_member_name(builder, "bridgeCalls");
  json_builder_add_int_value(builder, bridge_calls);
  json_builder_set_member_name(builder, "coreEvents");
  json_builder_add_int_value(builder, core_events);
  json_builder_set_member_name(builder, "networkRequestsLastMinute");
  json_builder_add_int_value(builder, network_requests);
  json_builder_set_member_name(builder, "logLinesLastMinute");
  json_builder_add_int_value(builder, log_lines);
  json_builder_set_member_name(builder, "measuredAt");
  json_builder_add_string_value(builder, measured_at);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static void append_bridge_call_row_object(JsonBuilder *builder, sqlite3_stmt *statement) {
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "bridgeCallId");
  json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
  json_builder_set_member_name(builder, "sessionId");
  sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
  json_builder_set_member_name(builder, "appId");
  sqlite3_column_text(statement, 2) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 2));
  json_builder_set_member_name(builder, "installId");
  sqlite3_column_text(statement, 3) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 3));
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 4));
  json_builder_set_member_name(builder, "params");
  json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 5));
  json_builder_set_member_name(builder, "result");
  json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 6));
  json_builder_set_member_name(builder, "error");
  json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 7));
  json_builder_set_member_name(builder, "durationMs");
  sqlite3_column_type(statement, 8) == SQLITE_NULL ? json_builder_add_null_value(builder) : json_builder_add_int_value(builder, sqlite3_column_int64(statement, 8));
  json_builder_set_member_name(builder, "createdAt");
  json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 9));
  json_builder_end_object(builder);
}

static void append_bridge_call_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql = app_id == NULL
      ? "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls ORDER BY created_at"
      : "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls WHERE app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      append_bridge_call_row_object(builder, statement);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static gchar *runtime_bridge_calls_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "bridgeCalls");
  append_bridge_call_rows(builder, db, app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static void append_core_event_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id);

static gchar *runtime_event_log_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "bridgeCalls");
  append_bridge_call_rows(builder, db, app_id);
  json_builder_set_member_name(builder, "coreEvents");
  append_core_event_rows(builder, db, app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static void append_core_event_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql = app_id == NULL
      ? "SELECT event_id, session_id, app_id, state_version_before, created_at FROM core_events ORDER BY created_at"
      : "SELECT event_id, session_id, app_id, state_version_before, created_at FROM core_events WHERE app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "eventId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "sessionId");
      sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 2) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 2));
      json_builder_set_member_name(builder, "stateVersionBefore");
      sqlite3_column_type(statement, 3) == SQLITE_NULL ? json_builder_add_null_value(builder) : json_builder_add_int_value(builder, sqlite3_column_int64(statement, 3));
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 4));
      json_builder_end_object(builder);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static void append_core_event_snapshot_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql =
      "SELECT event_id, session_id, app_id, install_id, state_version_before, event_json, created_at "
      "FROM core_events WHERE app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    bind_text(statement, 1, app_id);
    while (sqlite3_step(statement) == SQLITE_ROW) {
      const gchar *event_json = (const gchar *)sqlite3_column_text(statement, 5);
      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "eventId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "sessionId");
      sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 2) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 2));
      json_builder_set_member_name(builder, "installId");
      sqlite3_column_text(statement, 3) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 3));
      json_builder_set_member_name(builder, "stateVersionBefore");
      sqlite3_column_type(statement, 4) == SQLITE_NULL ? json_builder_add_null_value(builder) : json_builder_add_int_value(builder, sqlite3_column_int64(statement, 4));
      json_builder_set_member_name(builder, "eventJson");
      event_json == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, event_json);
      json_builder_set_member_name(builder, "event");
      json_builder_add_json_text_or_null(builder, event_json);
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 6));
      json_builder_end_object(builder);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static void append_core_action_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql =
      "SELECT action_id, event_id, session_id, app_id, action_json, created_at "
      "FROM core_actions WHERE app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    bind_text(statement, 1, app_id);
    while (sqlite3_step(statement) == SQLITE_ROW) {
      const gchar *action_json = (const gchar *)sqlite3_column_text(statement, 4);
      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "actionId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "eventId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "sessionId");
      sqlite3_column_text(statement, 2) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 2));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 3) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 3));
      json_builder_set_member_name(builder, "actionJson");
      action_json == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, action_json);
      json_builder_set_member_name(builder, "action");
      json_builder_add_json_text_or_null(builder, action_json);
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 5));
      json_builder_end_object(builder);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static gint64 core_state_version(sqlite3 *db, const gchar *app_id) {
  sqlite3_stmt *statement = NULL;
  gint64 state_version = 0;
  if (sqlite3_prepare_v2(
          db,
          "SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, app_id);
    if (sqlite3_step(statement) == SQLITE_ROW) {
      state_version = sqlite3_column_int64(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return state_version;
}

static gchar *runtime_core_snapshot_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "stateVersion");
  json_builder_add_int_value(builder, core_state_version(db, app_id));
  json_builder_set_member_name(builder, "coreEvents");
  append_core_event_snapshot_rows(builder, db, app_id);
  json_builder_set_member_name(builder, "coreActions");
  append_core_action_rows(builder, db, app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static gchar *runtime_assert_core_action_json(
    DevControlPlane *plane,
    const gchar *app_id,
    const gchar *expected_type,
    JsonNode *expected_match,
    JsonNode *expected_action,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not open platform database");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    return NULL;
  }

  sqlite3_stmt *statement = NULL;
  const gchar *sql = "SELECT action_json FROM core_actions WHERE app_id = ? ORDER BY created_at";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not read core action rows");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    platform_database_close(db);
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "count");
  gint64 count = 0;
  JsonBuilder *actions_builder = json_builder_new();
  json_builder_begin_array(actions_builder);

  bind_text(statement, 1, app_id);
  while (sqlite3_step(statement) == SQLITE_ROW) {
    const gchar *action_json = (const gchar *)sqlite3_column_text(statement, 0);
    if (action_json == NULL) {
      continue;
    }
    JsonParser *action_parser = json_parser_new();
    if (!json_parser_load_from_data(action_parser, action_json, -1, NULL)) {
      g_object_unref(action_parser);
      continue;
    }
    JsonNode *action_node = json_parser_get_root(action_parser);
    JsonObject *action_object = action_node != NULL && JSON_NODE_HOLDS_OBJECT(action_node) ? json_node_get_object(action_node) : NULL;
    gboolean matches = action_object != NULL;
    if (matches && expected_type != NULL) {
      JsonNode *type_node = json_object_get_member(action_object, "type");
      matches = type_node != NULL &&
          JSON_NODE_HOLDS_VALUE(type_node) &&
          json_node_get_value_type(type_node) == G_TYPE_STRING &&
          g_strcmp0(json_node_get_string(type_node), expected_type) == 0;
    }
    if (matches && expected_action != NULL) {
      matches = json_node_matches_subset(action_node, expected_action) && json_node_matches_subset(expected_action, action_node);
    }
    if (matches && expected_match != NULL) {
      matches = json_node_matches_subset(action_node, expected_match);
    }
    if (matches) {
      count++;
      json_builder_add_value(actions_builder, json_node_copy(action_node));
    }
    g_object_unref(action_parser);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);

  json_builder_end_array(actions_builder);
  JsonNode *actions_node = json_builder_get_root(actions_builder);
  json_builder_add_int_value(builder, count);
  json_builder_set_member_name(builder, "actions");
  json_builder_add_value(builder, actions_node);
  json_builder_end_object(builder);
  g_object_unref(actions_builder);

  if (count == 0) {
    g_object_unref(builder);
    *error_code = g_strdup("core_action.not_found");
    *error_message = g_strdup("Expected core action was not found");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static JsonObject *core_replay_params_for_event(JsonNode *event_node) {
  JsonObject *params = json_object_new();
  json_object_set_member(params, "event", json_node_copy(event_node));
  return params;
}

static gchar *runtime_replay_events_json(const gchar *app_id, JsonArray *events) {
  ZigCoreBridge replay_core = {0};
  zig_core_bridge_init(&replay_core);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "replay");
  json_builder_begin_array(builder);

  for (guint index = 0; index < json_array_get_length(events); index++) {
    JsonNode *event_node = json_array_get_element(events, index);
    JsonObject *params = core_replay_params_for_event(event_node);
    BridgeRequest request = {0};
    request.has_id = TRUE;
    request.id = g_strdup_printf("control_replay_%u", index);
    request.method = g_strdup("core.step");
    request.params = params;
    request.context.app_id = g_strdup(app_id);
    request.context.storage_prefix = g_strdup_printf("%s:", app_id);
    request.context.approved_permissions = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
    g_hash_table_add(request.context.approved_permissions, g_strdup("core.step"));
    request.context.mount_token = g_strdup("linux-control-replay");

    JsonNode *response = zig_core_bridge_step(&replay_core, &request);
    JsonObject *response_object = response != NULL && JSON_NODE_HOLDS_OBJECT(response) ? json_node_get_object(response) : NULL;

    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "index");
    json_builder_add_int_value(builder, index);
    json_builder_set_member_name(builder, "event");
    json_builder_add_value(builder, json_node_copy(event_node));
    json_builder_set_member_name(builder, "result");
    if (response_object != NULL && json_object_has_member(response_object, "result")) {
      json_builder_add_value(builder, json_node_copy(json_object_get_member(response_object, "result")));
    } else {
      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "ok");
      json_builder_add_boolean_value(builder, FALSE);
      json_builder_set_member_name(builder, "error");
      if (response_object != NULL && json_object_has_member(response_object, "error")) {
        json_builder_add_value(builder, json_node_copy(json_object_get_member(response_object, "error")));
      } else {
        json_builder_begin_object(builder);
        json_builder_set_member_name(builder, "code");
        json_builder_add_string_value(builder, "core_error");
        json_builder_set_member_name(builder, "message");
        json_builder_add_string_value(builder, "Replay event failed");
        json_builder_set_member_name(builder, "details");
        json_builder_begin_object(builder);
        json_builder_end_object(builder);
        json_builder_end_object(builder);
      }
      json_builder_set_member_name(builder, "actions");
      json_builder_begin_array(builder);
      json_builder_end_array(builder);
      json_builder_end_object(builder);
    }
    json_builder_end_object(builder);

    if (response != NULL) {
      json_node_unref(response);
    }
    bridge_request_clear(&request);
  }

  json_builder_end_array(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  zig_core_bridge_clear(&replay_core);
  return text;
}

static void append_console_log_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql = app_id == NULL
      ? "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at LIMIT 100"
      : "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at LIMIT 100";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      const gchar *params_json = (const gchar *)sqlite3_column_text(statement, 2);
      const gchar *level = NULL;
      const gchar *message = NULL;
      JsonParser *params_parser = json_parser_new();
      if (params_json != NULL && json_parser_load_from_data(params_parser, params_json, -1, NULL)) {
        JsonNode *root = json_parser_get_root(params_parser);
        if (root != NULL && JSON_NODE_HOLDS_OBJECT(root)) {
          JsonObject *params = json_node_get_object(root);
          level = object_string(params, "level", NULL);
          message = object_string(params, "message", NULL);
        }
      }

      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "bridgeCallId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "level");
      level == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, level);
      json_builder_set_member_name(builder, "message");
      message == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, message);
      json_builder_set_member_name(builder, "params");
      json_builder_add_json_text_or_null(builder, params_json);
      json_builder_set_member_name(builder, "result");
      json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 3));
      json_builder_set_member_name(builder, "error");
      json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 4));
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 5));
      json_builder_end_object(builder);
      g_object_unref(params_parser);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static const gchar *json_text_string_member(JsonParser *parser, const gchar *text, const gchar *member) {
  if (text == NULL || text[0] == '\0' || !json_parser_load_from_data(parser, text, -1, NULL)) {
    return NULL;
  }
  JsonNode *root = json_parser_get_root(parser);
  if (root == NULL || !JSON_NODE_HOLDS_OBJECT(root)) {
    return NULL;
  }
  JsonObject *object = json_node_get_object(root);
  if (!json_object_has_member(object, member)) {
    return NULL;
  }
  JsonNode *node = json_object_get_member(object, member);
  return node != NULL && JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) == G_TYPE_STRING
      ? json_node_get_string(node)
      : NULL;
}

static void append_notification_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql = app_id == NULL
      ? "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' ORDER BY created_at LIMIT 100"
      : "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' AND app_id = ? ORDER BY created_at LIMIT 100";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      const gchar *params_json = (const gchar *)sqlite3_column_text(statement, 2);
      JsonParser *message_parser = json_parser_new();
      JsonParser *level_parser = json_parser_new();
      const gchar *message = json_text_string_member(message_parser, params_json, "message");
      const gchar *level = json_text_string_member(level_parser, params_json, "level");

      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "bridgeCallId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "message");
      message == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, message);
      json_builder_set_member_name(builder, "level");
      level == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, level);
      json_builder_set_member_name(builder, "params");
      json_builder_add_json_text_or_null(builder, params_json);
      json_builder_set_member_name(builder, "result");
      json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 3));
      json_builder_set_member_name(builder, "error");
      json_builder_add_json_text_or_null(builder, (const gchar *)sqlite3_column_text(statement, 4));
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 5));
      json_builder_end_object(builder);
      g_object_unref(message_parser);
      g_object_unref(level_parser);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
}

static gchar *runtime_console_logs_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "logs");
  append_console_log_rows(builder, db, app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static gchar *notification_capture_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "notifications");
  append_notification_rows(builder, db, app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static gchar *assert_bridge_call_json(
    DevControlPlane *plane,
    const gchar *app_id,
    const gchar *method,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not open platform database");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    return NULL;
  }

  sqlite3_stmt *statement = NULL;
  const gchar *sql =
      "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at "
      "FROM bridge_calls WHERE app_id = ? AND method = ? ORDER BY created_at";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not read bridge call rows");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    platform_database_close(db);
    return NULL;
  }

  bind_text(statement, 1, app_id);
  bind_text(statement, 2, method);
  gint64 count = 0;
  JsonBuilder *latest_builder = NULL;
  while (sqlite3_step(statement) == SQLITE_ROW) {
    count++;
    if (latest_builder != NULL) {
      g_object_unref(latest_builder);
    }
    latest_builder = json_builder_new();
    append_bridge_call_row_object(latest_builder, statement);
  }
  sqlite3_finalize(statement);
  platform_database_close(db);

  if (count == 0 || latest_builder == NULL) {
    *error_code = g_strdup("assertion_failed");
    *error_message = g_strdup("Expected bridge call was not recorded");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  JsonNode *latest = json_builder_get_root(latest_builder);
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "method");
  json_builder_add_string_value(builder, method);
  json_builder_set_member_name(builder, "count");
  json_builder_add_int_value(builder, count);
  json_builder_set_member_name(builder, "latest");
  json_builder_add_value(builder, latest);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  g_object_unref(latest_builder);
  return text;
}

typedef struct {
  gchar *kind;
  gchar *value;
  gchar *tag;
} RuntimeUiMatch;

static void runtime_ui_match_free(gpointer data) {
  RuntimeUiMatch *match = data;
  if (match == NULL) {
    return;
  }
  g_free(match->kind);
  g_free(match->value);
  g_free(match->tag);
  g_free(match);
}

static RuntimeUiMatch *runtime_ui_match_new(const gchar *kind, const gchar *value, const gchar *tag) {
  RuntimeUiMatch *match = g_new0(RuntimeUiMatch, 1);
  match->kind = g_strdup(kind);
  match->value = g_strdup(value);
  match->tag = tag == NULL ? NULL : g_strdup(tag);
  return match;
}

static gchar *html_for_bundled_app(const gchar *app_id) {
  g_autofree gchar *manifest_path = app_sandbox_manifest_path_for_app(app_id);
  if (manifest_path == NULL) {
    return g_strdup("");
  }
  g_autofree gchar *app_dir = g_path_get_dirname(manifest_path);
  g_autofree gchar *index_path = g_build_filename(app_dir, "index.html", NULL);
  gchar *contents = NULL;
  if (!g_file_get_contents(index_path, &contents, NULL, NULL)) {
    return g_strdup("");
  }
  return contents;
}

static gchar *regex_replace_all(const gchar *text, const gchar *pattern, const gchar *replacement) {
  GError *error = NULL;
  GRegex *regex = g_regex_new(pattern, G_REGEX_CASELESS | G_REGEX_DOTALL | G_REGEX_MULTILINE, 0, &error);
  if (regex == NULL) {
    g_clear_error(&error);
    return g_strdup(text == NULL ? "" : text);
  }
  gchar *result = g_regex_replace(regex, text == NULL ? "" : text, -1, 0, replacement, 0, &error);
  g_regex_unref(regex);
  if (result == NULL) {
    g_clear_error(&error);
    return g_strdup(text == NULL ? "" : text);
  }
  return result;
}

static gchar *replace_all_literal(const gchar *text, const gchar *needle, const gchar *replacement) {
  const gchar *safe = text == NULL ? "" : text;
  if (needle == NULL || needle[0] == '\0') {
    return g_strdup(safe);
  }
  GString *out = g_string_new("");
  const gchar *cursor = safe;
  gsize needle_len = strlen(needle);
  while (TRUE) {
    const gchar *hit = strstr(cursor, needle);
    if (hit == NULL) {
      g_string_append(out, cursor);
      break;
    }
    g_string_append_len(out, cursor, hit - cursor);
    g_string_append(out, replacement == NULL ? "" : replacement);
    cursor = hit + needle_len;
  }
  return g_string_free(out, FALSE);
}

static gchar *collapse_whitespace(const gchar *text) {
  GString *out = g_string_new("");
  gboolean in_space = FALSE;
  for (const gchar *cursor = text == NULL ? "" : text; *cursor != '\0'; cursor++) {
    if (g_ascii_isspace(*cursor)) {
      if (out->len > 0 && !in_space) {
        g_string_append_c(out, ' ');
      }
      in_space = TRUE;
      continue;
    }
    g_string_append_c(out, *cursor);
    in_space = FALSE;
  }
  if (out->len > 0 && out->str[out->len - 1] == ' ') {
    g_string_truncate(out, out->len - 1);
  }
  return g_string_free(out, FALSE);
}

static gchar *html_text(const gchar *html) {
  g_autofree gchar *without_scripts = regex_replace_all(html, "<script\\b[^>]*>[\\s\\S]*?</script>", " ");
  g_autofree gchar *without_styles = regex_replace_all(without_scripts, "<style\\b[^>]*>[\\s\\S]*?</style>", " ");
  g_autofree gchar *without_tags = regex_replace_all(without_styles, "<[^>]+>", " ");
  g_autofree gchar *nbsp = replace_all_literal(without_tags, "&nbsp;", " ");
  g_autofree gchar *amp = replace_all_literal(nbsp, "&amp;", "&");
  g_autofree gchar *lt = replace_all_literal(amp, "&lt;", "<");
  g_autofree gchar *gt = replace_all_literal(lt, "&gt;", ">");
  g_autofree gchar *quot = replace_all_literal(gt, "&quot;", "\"");
  return collapse_whitespace(quot);
}

static gchar *first_match(const gchar *text, const gchar *pattern) {
  GError *error = NULL;
  GRegex *regex = g_regex_new(pattern, G_REGEX_CASELESS | G_REGEX_DOTALL | G_REGEX_MULTILINE, 0, &error);
  if (regex == NULL) {
    g_clear_error(&error);
    return g_strdup("");
  }
  GMatchInfo *match_info = NULL;
  gchar *result = NULL;
  if (g_regex_match(regex, text == NULL ? "" : text, 0, &match_info)) {
    result = g_match_info_fetch(match_info, 1);
  }
  g_match_info_free(match_info);
  g_regex_unref(regex);
  return result == NULL ? g_strdup("") : result;
}

static gboolean regex_contains(const gchar *text, const gchar *pattern) {
  GError *error = NULL;
  GRegex *regex = g_regex_new(pattern, G_REGEX_CASELESS | G_REGEX_DOTALL | G_REGEX_MULTILINE, 0, &error);
  if (regex == NULL) {
    g_clear_error(&error);
    return FALSE;
  }
  gboolean matched = g_regex_match(regex, text == NULL ? "" : text, 0, NULL);
  g_regex_unref(regex);
  return matched;
}

static gint compare_string_pointers(gconstpointer left, gconstpointer right) {
  const gchar *left_string = *(const gchar * const *)left;
  const gchar *right_string = *(const gchar * const *)right;
  return g_strcmp0(left_string, right_string);
}

static void append_test_id_array(JsonBuilder *builder, const gchar *html) {
  GRegex *regex = g_regex_new("\\bdata-testid=[\"']([^\"']+)[\"']", G_REGEX_CASELESS | G_REGEX_DOTALL | G_REGEX_MULTILINE, 0, NULL);
  GPtrArray *ids = g_ptr_array_new_with_free_func(g_free);
  GMatchInfo *match_info = NULL;
  if (regex != NULL && g_regex_match(regex, html == NULL ? "" : html, 0, &match_info)) {
    do {
      gchar *value = g_match_info_fetch(match_info, 1);
      if (value != NULL && value[0] != '\0') {
        g_ptr_array_add(ids, value);
      } else {
        g_free(value);
      }
    } while (g_match_info_next(match_info, NULL));
  }
  g_match_info_free(match_info);
  if (regex != NULL) {
    g_regex_unref(regex);
  }
  g_ptr_array_sort(ids, compare_string_pointers);

  json_builder_begin_array(builder);
  for (guint index = 0; index < ids->len; index++) {
    json_builder_add_string_value(builder, g_ptr_array_index(ids, index));
  }
  json_builder_end_array(builder);
  g_ptr_array_free(ids, TRUE);
}

static gchar *tag_for_attribute(const gchar *html, const gchar *attr, const gchar *value) {
  if (value == NULL || value[0] == '\0') {
    return NULL;
  }
  g_autofree gchar *escaped_attr = g_regex_escape_string(attr, -1);
  g_autofree gchar *escaped_value = g_regex_escape_string(value, -1);
  g_autofree gchar *pattern = g_strdup_printf("<([a-z0-9-]+)\\b[^>]*\\b%s=[\"']%s[\"'][^>]*>", escaped_attr, escaped_value);
  g_autofree gchar *tag = first_match(html, pattern);
  if (tag == NULL || tag[0] == '\0') {
    return NULL;
  }
  return g_ascii_strdown(tag, -1);
}

static gchar *test_id_selector_value(const gchar *selector) {
  return first_match(selector, "\\[data-testid=[\"']([^\"']+)[\"']\\]");
}

static gboolean is_simple_tag_selector(const gchar *selector) {
  return selector != NULL && g_regex_match_simple("^[a-z][a-z0-9-]*$", selector, G_REGEX_CASELESS, 0);
}

static GPtrArray *runtime_query_matches(const gchar *html, JsonObject *args) {
  GPtrArray *matches = g_ptr_array_new_with_free_func(runtime_ui_match_free);
  const gchar *test_id = object_string(args, "testId", NULL);
  if (test_id != NULL && test_id[0] != '\0') {
    g_autofree gchar *tag = tag_for_attribute(html, "data-testid", test_id);
    if (tag != NULL) {
      g_ptr_array_add(matches, runtime_ui_match_new("testId", test_id, tag));
    }
    return matches;
  }

  const gchar *selector = object_string(args, "selector", NULL);
  if (selector != NULL && selector[0] == '#' && selector[1] != '\0') {
    g_autofree gchar *tag = tag_for_attribute(html, "id", selector + 1);
    if (tag != NULL) {
      g_ptr_array_add(matches, runtime_ui_match_new("selector", selector, tag));
    }
    return matches;
  }

  if (selector != NULL && selector[0] != '\0') {
    g_autofree gchar *selector_test_id = test_id_selector_value(selector);
    if (selector_test_id != NULL && selector_test_id[0] != '\0') {
      g_autofree gchar *tag = tag_for_attribute(html, "data-testid", selector_test_id);
      if (tag != NULL) {
        g_ptr_array_add(matches, runtime_ui_match_new("selector", selector, tag));
      }
      return matches;
    }
  }

  const gchar *text = object_string(args, "text", NULL);
  if (text != NULL && text[0] != '\0') {
    g_autofree gchar *visible_text = html_text(html);
    if (strstr(visible_text, text) != NULL) {
      g_ptr_array_add(matches, runtime_ui_match_new("text", text, NULL));
    }
    return matches;
  }

  if (is_simple_tag_selector(selector)) {
    g_autofree gchar *escaped_selector = g_regex_escape_string(selector, -1);
    g_autofree gchar *pattern = g_strdup_printf("<%s\\b", escaped_selector);
    if (regex_contains(html, pattern)) {
      g_autofree gchar *tag = g_ascii_strdown(selector, -1);
      g_ptr_array_add(matches, runtime_ui_match_new("selector", selector, tag));
    }
  }
  return matches;
}

static void append_runtime_ui_match_object(JsonBuilder *builder, RuntimeUiMatch *match) {
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "kind");
  json_builder_add_string_value(builder, match->kind);
  json_builder_set_member_name(builder, "value");
  json_builder_add_string_value(builder, match->value);
  if (match->tag != NULL) {
    json_builder_set_member_name(builder, "tag");
    json_builder_add_string_value(builder, match->tag);
  }
  json_builder_end_object(builder);
}

static void append_runtime_ui_matches(JsonBuilder *builder, GPtrArray *matches) {
  json_builder_begin_array(builder);
  for (guint index = 0; index < matches->len; index++) {
    append_runtime_ui_match_object(builder, g_ptr_array_index(matches, index));
  }
  json_builder_end_array(builder);
}

static gchar *runtime_query_json(const gchar *app_id, JsonObject *args) {
  g_autofree gchar *html = html_for_bundled_app(app_id);
  const gchar *test_id = object_string(args, "testId", NULL);
  const gchar *selector = object_string(args, "selector", NULL);
  const gchar *text = object_string(args, "text", NULL);
  g_autofree gchar *test_id_query = test_id == NULL ? NULL : g_strdup_printf("[data-testid=\"%s\"]", test_id);
  const gchar *query = test_id_query != NULL ? test_id_query : selector != NULL ? selector : text != NULL ? text : "";
  GPtrArray *matches = runtime_query_matches(html, args);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, matches->len > 0);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "query");
  json_builder_add_string_value(builder, query);
  json_builder_set_member_name(builder, "matches");
  append_runtime_ui_matches(builder, matches);
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  g_ptr_array_free(matches, TRUE);
  return result;
}

static gchar *runtime_screenshot_json(const gchar *app_id, const gchar *label) {
  g_autofree gchar *html = html_for_bundled_app(app_id);
  g_autofree gchar *text = html_text(html);
  g_autofree gchar *raw_title = first_match(html, "<title[^>]*>([\\s\\S]*?)</title>");
  g_autofree gchar *title = html_text(raw_title);
  g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, text, -1);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "label");
  label == NULL || label[0] == '\0' ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, label);
  json_builder_set_member_name(builder, "format");
  json_builder_add_string_value(builder, "static-html-summary");
  json_builder_set_member_name(builder, "title");
  json_builder_add_string_value(builder, title);
  json_builder_set_member_name(builder, "textHash");
  g_autofree gchar *text_hash = g_strdup_printf("sha256:%s", hash);
  json_builder_add_string_value(builder, text_hash);
  json_builder_set_member_name(builder, "testIds");
  append_test_id_array(builder, html);
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  return result;
}

static gchar *runtime_target_command_json(
    const gchar *tool,
    JsonObject *args,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  if (g_strcmp0(tool, "runtime.press_key") == 0) {
    const gchar *key = object_string(args, "key", NULL);
    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "ok");
    json_builder_add_boolean_value(builder, TRUE);
    json_builder_set_member_name(builder, "key");
    key == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, key);
    json_builder_end_object(builder);
    gchar *result = json_builder_to_text(builder);
    g_object_unref(builder);
    return result;
  }

  const gchar *app_id = object_string(args, "appId", "");
  g_autofree gchar *html = html_for_bundled_app(app_id);
  GPtrArray *matches = runtime_query_matches(html, args);
  if (matches->len == 0) {
    g_ptr_array_free(matches, TRUE);
    *error_code = g_strdup("selector.not_found");
    *error_message = g_strdup("Runtime target was not found in generated app HTML");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "tool");
  json_builder_add_string_value(builder, tool);
  json_builder_set_member_name(builder, "target");
  append_runtime_ui_match_object(builder, g_ptr_array_index(matches, 0));
  if (g_strcmp0(tool, "runtime.type") == 0 || g_strcmp0(tool, "runtime.set_value") == 0) {
    json_builder_set_member_name(builder, "value");
    json_builder_add_string_value(builder, object_string_any(args, "value", "text", NULL, ""));
  }
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  g_ptr_array_free(matches, TRUE);
  return result;
}

static gchar *runtime_assert_visible_json(
    const gchar *app_id,
    JsonObject *args,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  g_autofree gchar *html = html_for_bundled_app(app_id);
  GPtrArray *matches = runtime_query_matches(html, args);
  if (matches->len == 0) {
    g_ptr_array_free(matches, TRUE);
    *error_code = g_strdup("selector.not_found");
    *error_message = g_strdup("Expected runtime target is not visible");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "matches");
  json_builder_add_int_value(builder, matches->len);
  json_builder_set_member_name(builder, "target");
  append_runtime_ui_match_object(builder, g_ptr_array_index(matches, 0));
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  g_ptr_array_free(matches, TRUE);
  return result;
}

static gchar *runtime_assert_text_json(
    const gchar *app_id,
    const gchar *text,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  g_autofree gchar *html = html_for_bundled_app(app_id);
  g_autofree gchar *visible_text = html_text(html);
  if (strstr(visible_text, text) == NULL) {
    *error_code = g_strdup("text.not_found");
    *error_message = g_strdup("Expected text was not found in installed package HTML");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "text");
  json_builder_add_string_value(builder, text);
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  return result;
}

static gchar *runtime_wait_for_json(
    DevControlPlane *plane,
    JsonObject *args,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  const gchar *kind = object_string(args, "kind", "idle");
  if (g_strcmp0(kind, "idle") == 0) {
    return g_strdup("{\"ok\":true,\"kind\":\"idle\"}");
  }

  if (g_strcmp0(kind, "bridge_call") == 0 || g_strcmp0(kind, "bridgeCall") == 0) {
    const gchar *app_id = object_string(args, "appId", "");
    const gchar *bridge_method = object_string(args, "method", "");
    g_autofree gchar *result = assert_bridge_call_json(plane, app_id, bridge_method, error_code, error_message, status);
    if (result == NULL) {
      if (g_strcmp0(*error_code, "assertion_failed") == 0) {
        g_free(*error_code);
        g_free(*error_message);
        *error_code = g_strdup("wait_timeout");
        *error_message = g_strdup("Expected bridge call was not recorded");
      }
      return NULL;
    }
    g_autofree gchar *prefix = g_str_has_suffix(result, "}") ? g_strndup(result, strlen(result) - 1) : g_strdup(result);
    g_autofree gchar *escaped_kind = json_escape(kind);
    return g_strdup_printf("%s,\"kind\":\"%s\"}", prefix, escaped_kind);
  }

  const gchar *app_id = object_string(args, "appId", "");
  g_autofree gchar *html = html_for_bundled_app(app_id);
  GPtrArray *matches = runtime_query_matches(html, args);
  if (matches->len == 0) {
    g_ptr_array_free(matches, TRUE);
    *error_code = g_strdup("wait_timeout");
    *error_message = g_strdup("Expected runtime condition did not appear");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "kind");
  json_builder_add_string_value(builder, kind);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "matches");
  json_builder_add_int_value(builder, matches->len);
  json_builder_end_object(builder);
  gchar *result = json_builder_to_text(builder);
  g_object_unref(builder);
  g_ptr_array_free(matches, TRUE);
  return result;
}

static gchar *runtime_timer_advance_json(JsonObject *args) {
  gint64 milliseconds = object_int_any(args, "ms", "milliseconds", NULL, 0);
  if (milliseconds < 0) {
    milliseconds = 0;
  }
  return g_strdup_printf("{\"ok\":true,\"advancedMs\":%" G_GINT64_FORMAT "}", milliseconds);
}

static gboolean console_log_row_is_error(const gchar *params_json, const gchar *error_json) {
  JsonParser *error_parser = json_parser_new();
  gboolean has_error = error_json != NULL &&
      error_json[0] != '\0' &&
      json_parser_load_from_data(error_parser, error_json, -1, NULL) &&
      json_parser_get_root(error_parser) != NULL &&
      json_node_get_node_type(json_parser_get_root(error_parser)) != JSON_NODE_NULL;
  g_object_unref(error_parser);
  if (has_error) {
    return TRUE;
  }

  JsonParser *params_parser = json_parser_new();
  const gchar *level = json_text_string_member(params_parser, params_json, "level");
  gboolean is_error = g_strcmp0(level, "error") == 0;
  g_object_unref(params_parser);
  return is_error;
}

static gchar *assert_no_console_errors_json(
    DevControlPlane *plane,
    const gchar *app_id,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not open platform database");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    return NULL;
  }

  const gchar *sql = app_id == NULL
      ? "SELECT params_json, error_json FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at"
      : "SELECT params_json, error_json FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not read console log rows");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    platform_database_close(db);
    return NULL;
  }
  if (app_id != NULL) {
    bind_text(statement, 1, app_id);
  }

  gint64 errors = 0;
  while (sqlite3_step(statement) == SQLITE_ROW) {
    if (console_log_row_is_error((const gchar *)sqlite3_column_text(statement, 0), (const gchar *)sqlite3_column_text(statement, 1))) {
      errors++;
    }
  }
  sqlite3_finalize(statement);
  platform_database_close(db);

  if (errors > 0) {
    *error_code = g_strdup("console_errors_found");
    *error_message = g_strdup("Console error logs were found");
    *status = SOUP_STATUS_BAD_REQUEST;
    return NULL;
  }
  return g_strdup("{\"ok\":true,\"errors\":0}");
}

typedef struct {
  const gchar *table;
  const gchar * const *columns;
  gsize column_count;
  const gchar *order_by;
  const gchar *filter_column;
} SafeDbTable;

static const gchar * const db_apps_columns[] = {"id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"};
static const gchar * const db_app_versions_columns[] = {"install_id", "app_id", "version", "runtime_version", "data_version", "manifest_json", "manifest_hash", "content_hash", "signature_json", "trust_level", "status", "created_at", "activated_at"};
static const gchar * const db_app_files_columns[] = {"install_id", "path", "content_text", "content_hash", "size_bytes", "mime", "created_at"};
static const gchar * const db_app_permissions_columns[] = {"install_id", "app_id", "permission", "requested", "approved", "approved_at", "reason"};
static const gchar * const db_app_storage_columns[] = {"app_id", "key", "value_json", "updated_at"};
static const gchar * const db_bridge_calls_columns[] = {"bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"};
static const gchar * const db_core_events_columns[] = {"event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"};
static const gchar * const db_core_actions_columns[] = {"action_id", "event_id", "session_id", "app_id", "action_json", "created_at"};
static const gchar * const db_test_runs_columns[] = {"test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"};
static const gchar * const db_control_sessions_columns[] = {"control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"};
static const gchar * const db_control_commands_columns[] = {"command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "args_json", "result_json", "error_json", "created_at", "duration_ms"};
static const gchar * const db_runtime_sessions_columns[] = {"session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"};
static const gchar * const db_runtime_snapshots_columns[] = {"snapshot_id", "session_id", "app_id", "install_id", "type", "snapshot_json", "content_hash", "created_at"};
static const gchar * const db_app_migrations_columns[] = {"migration_id", "app_id", "from_data_version", "to_data_version", "migration_json", "content_hash", "created_at"};
static const gchar * const db_app_install_reports_columns[] = {"report_id", "app_id", "install_id", "status", "validation_json", "security_json", "permissions_json", "compatibility_json", "smoke_test_json", "content_hash", "created_at"};
static const gchar * const db_backup_exports_columns[] = {"export_id", "type", "source_platform", "runtime_version", "content_hash", "created_at", "imported_at"};

static const SafeDbTable safe_db_apps = {"apps", db_apps_columns, G_N_ELEMENTS(db_apps_columns), "id", NULL};
static const SafeDbTable safe_db_app_versions = {"app_versions", db_app_versions_columns, G_N_ELEMENTS(db_app_versions_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_app_files = {"app_files", db_app_files_columns, G_N_ELEMENTS(db_app_files_columns), "path", NULL};
static const SafeDbTable safe_db_app_permissions = {"app_permissions", db_app_permissions_columns, G_N_ELEMENTS(db_app_permissions_columns), "permission", NULL};
static const SafeDbTable safe_db_app_storage = {"app_storage", db_app_storage_columns, G_N_ELEMENTS(db_app_storage_columns), "app_id, key", "app_id"};
static const SafeDbTable safe_db_bridge_calls = {"bridge_calls", db_bridge_calls_columns, G_N_ELEMENTS(db_bridge_calls_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_core_events = {"core_events", db_core_events_columns, G_N_ELEMENTS(db_core_events_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_core_actions = {"core_actions", db_core_actions_columns, G_N_ELEMENTS(db_core_actions_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_test_runs = {"test_runs", db_test_runs_columns, G_N_ELEMENTS(db_test_runs_columns), "started_at", "app_id"};
static const SafeDbTable safe_db_control_sessions = {"control_sessions", db_control_sessions_columns, G_N_ELEMENTS(db_control_sessions_columns), "started_at", NULL};
static const SafeDbTable safe_db_control_commands = {"control_commands", db_control_commands_columns, G_N_ELEMENTS(db_control_commands_columns), "created_at", NULL};
static const SafeDbTable safe_db_runtime_sessions = {"runtime_sessions", db_runtime_sessions_columns, G_N_ELEMENTS(db_runtime_sessions_columns), "started_at", NULL};
static const SafeDbTable safe_db_runtime_snapshots = {"runtime_snapshots", db_runtime_snapshots_columns, G_N_ELEMENTS(db_runtime_snapshots_columns), "created_at", NULL};
static const SafeDbTable safe_db_app_migrations = {"app_migrations", db_app_migrations_columns, G_N_ELEMENTS(db_app_migrations_columns), "created_at", NULL};
static const SafeDbTable safe_db_app_install_reports = {"app_install_reports", db_app_install_reports_columns, G_N_ELEMENTS(db_app_install_reports_columns), "created_at", NULL};
static const SafeDbTable safe_db_backup_exports = {"backup_exports", db_backup_exports_columns, G_N_ELEMENTS(db_backup_exports_columns), "created_at", NULL};

static const SafeDbTable * const db_snapshot_tables[] = {
    &safe_db_apps,
    &safe_db_app_versions,
    &safe_db_app_files,
    &safe_db_app_permissions,
    &safe_db_app_storage,
    &safe_db_app_migrations,
    &safe_db_app_install_reports,
    &safe_db_bridge_calls,
    &safe_db_core_events,
    &safe_db_core_actions,
    &safe_db_test_runs,
    &safe_db_control_sessions,
    &safe_db_control_commands,
    &safe_db_runtime_sessions,
    &safe_db_runtime_snapshots,
    &safe_db_backup_exports,
};

static void append_sqlite_value(JsonBuilder *builder, sqlite3_stmt *statement, int column) {
  switch (sqlite3_column_type(statement, column)) {
    case SQLITE_NULL:
      json_builder_add_null_value(builder);
      break;
    case SQLITE_INTEGER:
      json_builder_add_int_value(builder, sqlite3_column_int64(statement, column));
      break;
    case SQLITE_FLOAT:
      json_builder_add_double_value(builder, sqlite3_column_double(statement, column));
      break;
    case SQLITE_BLOB: {
      const guchar *blob = sqlite3_column_blob(statement, column);
      int bytes = sqlite3_column_bytes(statement, column);
      g_autofree gchar *encoded = blob == NULL || bytes <= 0 ? g_strdup("") : g_base64_encode(blob, (gsize)bytes);
      json_builder_add_string_value(builder, encoded);
      break;
    }
    case SQLITE_TEXT:
    default: {
      const gchar *text = (const gchar *)sqlite3_column_text(statement, column);
      text == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, text);
      break;
    }
  }
}

static void append_safe_table_rows(JsonBuilder *builder, sqlite3 *db, const SafeDbTable *spec, const gchar *filter_value) {
  json_builder_begin_array(builder);
  if (db == NULL || spec == NULL || spec->column_count == 0) {
    json_builder_end_array(builder);
    return;
  }

  gboolean has_filter = spec->filter_column != NULL && filter_value != NULL && filter_value[0] != '\0';
  GString *sql = g_string_new("SELECT ");
  for (gsize index = 0; index < spec->column_count; index++) {
    if (index > 0) {
      g_string_append(sql, ", ");
    }
    g_string_append(sql, spec->columns[index]);
  }
  g_string_append_printf(sql, " FROM %s", spec->table);
  if (has_filter) {
    g_string_append_printf(sql, " WHERE %s = ?", spec->filter_column);
  }
  g_string_append_printf(sql, " ORDER BY %s LIMIT 100", spec->order_by);

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, sql->str, -1, &statement, NULL) == SQLITE_OK) {
    if (has_filter) {
      bind_text(statement, 1, filter_value);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      json_builder_begin_object(builder);
      for (gsize index = 0; index < spec->column_count; index++) {
        json_builder_set_member_name(builder, spec->columns[index]);
        append_sqlite_value(builder, statement, (int)index);
      }
      json_builder_end_object(builder);
    }
  }
  sqlite3_finalize(statement);
  g_string_free(sql, TRUE);
  json_builder_end_array(builder);
}

static gchar *safe_table_rows_json(sqlite3 *db, const SafeDbTable *spec, const gchar *filter_value) {
  JsonBuilder *builder = json_builder_new();
  append_safe_table_rows(builder, db, spec, filter_value);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *snapshot_storage_rows_json(sqlite3 *db, const gchar *app_id) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_array(builder);
  if (db != NULL && app_id != NULL && app_id[0] != '\0') {
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(
            db,
            "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key",
            -1,
            &statement,
            NULL) == SQLITE_OK) {
      bind_text(statement, 1, app_id);
      while (sqlite3_step(statement) == SQLITE_ROW) {
        json_builder_begin_object(builder);
        json_builder_set_member_name(builder, "app_id");
        append_sqlite_value(builder, statement, 0);
        json_builder_set_member_name(builder, "key");
        append_sqlite_value(builder, statement, 1);
        json_builder_set_member_name(builder, "value_json");
        append_sqlite_value(builder, statement, 2);
        json_builder_set_member_name(builder, "updated_at");
        append_sqlite_value(builder, statement, 3);
        json_builder_end_object(builder);
      }
    }
    sqlite3_finalize(statement);
  }
  json_builder_end_array(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gboolean is_db_inspection_tool(const gchar *tool) {
  return g_strcmp0(tool, "db.snapshot") == 0 ||
         g_strcmp0(tool, "db.query_app_storage") == 0 ||
         g_strcmp0(tool, "db.query_app_versions") == 0 ||
         g_strcmp0(tool, "db.query_bridge_calls") == 0 ||
         g_strcmp0(tool, "db.query_core_events") == 0 ||
         g_strcmp0(tool, "db.query_test_runs") == 0;
}

static gboolean db_tool_requires_app_id(const gchar *tool) {
  return g_strcmp0(tool, "db.query_app_storage") == 0 ||
         g_strcmp0(tool, "db.query_app_versions") == 0;
}

static const SafeDbTable *safe_db_table_for_tool(const gchar *tool) {
  if (g_strcmp0(tool, "db.query_app_storage") == 0) {
    return &safe_db_app_storage;
  }
  if (g_strcmp0(tool, "db.query_app_versions") == 0) {
    return &safe_db_app_versions;
  }
  if (g_strcmp0(tool, "db.query_bridge_calls") == 0) {
    return &safe_db_bridge_calls;
  }
  if (g_strcmp0(tool, "db.query_core_events") == 0) {
    return &safe_db_core_events;
  }
  if (g_strcmp0(tool, "db.query_test_runs") == 0) {
    return &safe_db_test_runs;
  }
  return NULL;
}

static gchar *db_snapshot_json(DevControlPlane *plane, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  for (gsize index = 0; index < G_N_ELEMENTS(db_snapshot_tables); index++) {
    const SafeDbTable *spec = db_snapshot_tables[index];
    json_builder_set_member_name(builder, spec->table);
    append_safe_table_rows(builder, db, spec, NULL);
  }
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  platform_database_close(db);
  return text;
}

static gchar *db_export_document_json(DevControlPlane *plane, const gchar *export_type, gboolean include_debug, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *export_id = make_id("export");
  g_autofree gchar *created_at = now_iso();
  g_autofree gchar *capabilities = runtime_capabilities_json(plane, NULL);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "exportId");
  json_builder_add_string_value(builder, export_id);
  json_builder_set_member_name(builder, "type");
  json_builder_add_string_value(builder, export_type);
  json_builder_set_member_name(builder, "createdAt");
  json_builder_add_string_value(builder, created_at);
  json_builder_set_member_name(builder, "runtimeVersion");
  json_builder_add_string_value(builder, "0.4.0");
  json_builder_set_member_name(builder, "source");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "platform");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, "linux-native");
  json_builder_end_object(builder);
  json_builder_set_member_name(builder, "apps");
  append_safe_table_rows(builder, db, &safe_db_apps, NULL);
  json_builder_set_member_name(builder, "appVersions");
  append_safe_table_rows(builder, db, &safe_db_app_versions, NULL);
  json_builder_set_member_name(builder, "appFiles");
  append_safe_table_rows(builder, db, &safe_db_app_files, NULL);
  json_builder_set_member_name(builder, "appPermissions");
  append_safe_table_rows(builder, db, &safe_db_app_permissions, NULL);
  json_builder_set_member_name(builder, "appStorage");
  append_safe_table_rows(builder, db, &safe_db_app_storage, NULL);
  json_builder_set_member_name(builder, "appMigrations");
  append_safe_table_rows(builder, db, &safe_db_app_migrations, NULL);
  json_builder_set_member_name(builder, "appInstallReports");
  append_safe_table_rows(builder, db, &safe_db_app_install_reports, NULL);
  json_builder_set_member_name(builder, "runtimeCapabilities");
  json_builder_add_json_text_or_null(builder, capabilities);
  json_builder_set_member_name(builder, "debug");
  json_builder_begin_object(builder);
  if (include_debug) {
    json_builder_set_member_name(builder, "runtimeSessions");
    append_safe_table_rows(builder, db, &safe_db_runtime_sessions, NULL);
    json_builder_set_member_name(builder, "bridgeCalls");
    append_safe_table_rows(builder, db, &safe_db_bridge_calls, NULL);
    json_builder_set_member_name(builder, "controlSessions");
    append_safe_table_rows(builder, db, &safe_db_control_sessions, NULL);
    json_builder_set_member_name(builder, "controlCommands");
    append_safe_table_rows(builder, db, &safe_db_control_commands, NULL);
    json_builder_set_member_name(builder, "coreEvents");
    append_safe_table_rows(builder, db, &safe_db_core_events, NULL);
    json_builder_set_member_name(builder, "coreActions");
    append_safe_table_rows(builder, db, &safe_db_core_actions, NULL);
    json_builder_set_member_name(builder, "runtimeSnapshots");
    append_safe_table_rows(builder, db, &safe_db_runtime_snapshots, NULL);
    json_builder_set_member_name(builder, "testRuns");
    append_safe_table_rows(builder, db, &safe_db_test_runs, NULL);
  }
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  g_autofree gchar *without_hash = json_builder_to_text(builder);
  g_object_unref(builder);

  g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, without_hash, -1);
  g_autofree gchar *content_hash = g_strdup_printf("sha256:%s", hash);
  gsize without_hash_len = strlen(without_hash);
  g_autofree gchar *escaped_hash = json_escape(content_hash);
  g_autofree gchar *document = g_strdup_printf(
      "%.*s,\"contentHash\":\"%s\"}",
      (int)(without_hash_len > 0 ? without_hash_len - 1 : 0),
      without_hash,
      escaped_hash);

  sqlite3_stmt *statement = NULL;
  gboolean ok = sqlite3_prepare_v2(
      db,
      "INSERT OR REPLACE INTO backup_exports "
      "(export_id, type, source_platform, runtime_version, export_json, content_hash, created_at) "
      "VALUES (?, ?, 'linux', '0.4.0', ?, ?, ?)",
      -1,
      &statement,
      NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, export_id);
    bind_text(statement, 2, export_type);
    bind_text(statement, 3, document);
    bind_text(statement, 4, content_hash);
    bind_text(statement, 5, created_at);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  platform_database_close(db);
  if (!ok) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not record %s export", export_type);
    return NULL;
  }

  return g_steal_pointer(&document);
}

static gchar *db_export_backup_json(DevControlPlane *plane, GError **error) {
  return db_export_document_json(plane, "backup", FALSE, error);
}

static gchar *db_export_debug_bundle_json(DevControlPlane *plane, GError **error) {
  return db_export_document_json(plane, "debug-bundle", TRUE, error);
}

static gchar *db_import_backup_json(DevControlPlane *plane, JsonObject *document, JsonNode *document_node, GError **error) {
  const gchar *type = object_string(document, "type", "");
  if (g_strcmp0(type, "backup") != 0 && g_strcmp0(type, "debug-bundle") != 0 && g_strcmp0(type, "test-fixture") != 0) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Backup import requires type backup, debug-bundle, or test-fixture");
    return NULL;
  }

  JsonArray *apps = object_array(document, "apps");
  JsonArray *versions = object_array(document, "appVersions");
  JsonArray *files = object_array(document, "appFiles");
  JsonArray *permissions = object_array(document, "appPermissions");
  JsonArray *storage_rows = object_array(document, "appStorage");
  if (apps == NULL || versions == NULL || files == NULL || permissions == NULL || storage_rows == NULL) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Backup import document is missing required arrays");
    return NULL;
  }
  JsonArray *migrations = object_array(document, "appMigrations");
  JsonArray *reports = object_array(document, "appInstallReports");

  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *created_at = now_iso();
  char *sql_error = NULL;
  if (sqlite3_exec(db, "BEGIN IMMEDIATE", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not start backup import transaction");
    sqlite3_free(sql_error);
    platform_database_close(db);
    return NULL;
  }

  gboolean ok = TRUE;

  for (guint index = 0; ok && index < json_array_get_length(apps); index++) {
    JsonObject *app = NULL;
    ok = json_array_object_at(apps, index, &app);
    const gchar *app_id = ok ? object_string_any(app, "id", "appId", NULL, NULL) : NULL;
    if (app_id == NULL || app_id[0] == '\0') {
      ok = FALSE;
      break;
    }
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, app_id);
      bind_text(statement, 2, object_string(app, "name", app_id));
      bind_text(statement, 3, object_string(app, "status", "enabled"));
      bind_nullable_text(statement, 4, object_string_any(app, "active_install_id", "activeInstallId", NULL, NULL));
      bind_nullable_text(statement, 5, object_string_any(app, "active_version", "activeVersion", NULL, NULL));
      sqlite3_bind_int64(statement, 6, object_int_any(app, "data_version", "dataVersion", NULL, 1));
      bind_text(statement, 7, object_string_any(app, "created_at", "createdAt", NULL, created_at));
      bind_text(statement, 8, object_string_any(app, "updated_at", "updatedAt", NULL, created_at));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && index < json_array_get_length(versions); index++) {
    JsonObject *version = NULL;
    ok = json_array_object_at(versions, index, &version);
    const gchar *install_id = ok ? object_string_any(version, "install_id", "installId", NULL, NULL) : NULL;
    const gchar *app_id = ok ? object_string_any(version, "app_id", "appId", NULL, NULL) : NULL;
    const gchar *app_version = ok ? object_string_any(version, "version", "appVersion", NULL, NULL) : NULL;
    if (install_id == NULL || install_id[0] == '\0' || app_id == NULL || app_id[0] == '\0' || app_version == NULL || app_version[0] == '\0') {
      ok = FALSE;
      break;
    }
    g_autofree gchar *manifest_json = object_json_text_any(version, "manifest_json", "manifestJson", "manifest", "{}");
    g_autofree gchar *signature_json = object_json_text_any(version, "signature_json", "signatureJson", "signature", NULL);
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, install_id);
      bind_text(statement, 2, app_id);
      bind_text(statement, 3, app_version);
      bind_text(statement, 4, object_string_any(version, "runtime_version", "runtimeVersion", NULL, "0.1.0"));
      sqlite3_bind_int64(statement, 5, object_int_any(version, "data_version", "dataVersion", NULL, 1));
      bind_text(statement, 6, manifest_json);
      bind_text(statement, 7, object_string_any(version, "manifest_hash", "manifestHash", NULL, ""));
      bind_text(statement, 8, object_string_any(version, "content_hash", "contentHash", NULL, ""));
      bind_nullable_text(statement, 9, signature_json);
      bind_text(statement, 10, object_string_any(version, "trust_level", "trustLevel", NULL, "developer"));
      bind_text(statement, 11, object_string(version, "status", "installed"));
      bind_text(statement, 12, object_string_any(version, "created_at", "installedAt", "createdAt", created_at));
      bind_nullable_text(statement, 13, object_string_any(version, "activated_at", "activatedAt", NULL, NULL));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && index < json_array_get_length(files); index++) {
    JsonObject *file = NULL;
    ok = json_array_object_at(files, index, &file);
    const gchar *install_id = ok ? object_string_any(file, "install_id", "installId", NULL, NULL) : NULL;
    const gchar *path_value = ok ? object_string(file, "path", NULL) : NULL;
    if (install_id == NULL || install_id[0] == '\0' || path_value == NULL || path_value[0] == '\0') {
      ok = FALSE;
      break;
    }
    const gchar *content_text = object_string_any(file, "content_text", "contentText", NULL, "");
    const gchar *content_hash = object_string_any(file, "content_hash", "contentHash", NULL, NULL);
    g_autofree gchar *computed_hash = NULL;
    if (content_hash == NULL || content_hash[0] == '\0') {
      g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, content_text, -1);
      computed_hash = g_strdup_printf("sha256:%s", hash);
      content_hash = computed_hash;
    }
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, install_id);
      bind_text(statement, 2, path_value);
      bind_text(statement, 3, content_text);
      bind_text(statement, 4, content_hash);
      sqlite3_bind_int64(statement, 5, object_int_any(file, "size_bytes", "sizeBytes", NULL, (gint64)strlen(content_text)));
      bind_text(statement, 6, object_string(file, "mime", "text/plain"));
      bind_text(statement, 7, object_string_any(file, "created_at", "createdAt", NULL, created_at));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && index < json_array_get_length(permissions); index++) {
    JsonObject *permission = NULL;
    ok = json_array_object_at(permissions, index, &permission);
    const gchar *install_id = ok ? object_string_any(permission, "install_id", "installId", NULL, NULL) : NULL;
    const gchar *app_id = ok ? object_string_any(permission, "app_id", "appId", NULL, NULL) : NULL;
    const gchar *permission_name = ok ? object_string(permission, "permission", NULL) : NULL;
    if (install_id == NULL || install_id[0] == '\0' || app_id == NULL || app_id[0] == '\0' || permission_name == NULL || permission_name[0] == '\0') {
      ok = FALSE;
      break;
    }
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, install_id);
      bind_text(statement, 2, app_id);
      bind_text(statement, 3, permission_name);
      sqlite3_bind_int64(statement, 4, object_int_any(permission, "requested", NULL, NULL, 1));
      sqlite3_bind_int64(statement, 5, object_int_any(permission, "approved", NULL, NULL, 0));
      bind_nullable_text(statement, 6, object_string_any(permission, "approved_at", "approvedAt", NULL, NULL));
      bind_nullable_text(statement, 7, object_string(permission, "reason", NULL));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && index < json_array_get_length(storage_rows); index++) {
    JsonObject *storage = NULL;
    ok = json_array_object_at(storage_rows, index, &storage);
    const gchar *app_id = ok ? object_string_any(storage, "app_id", "appId", NULL, NULL) : NULL;
    const gchar *key = ok ? object_string(storage, "key", NULL) : NULL;
    if (app_id == NULL || app_id[0] == '\0' || key == NULL || key[0] == '\0') {
      ok = FALSE;
      break;
    }
    g_autofree gchar *value_json = object_json_text_any(storage, "value_json", "valueJson", "value", "null");
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, app_id);
      bind_text(statement, 2, key);
      bind_text(statement, 3, value_json);
      bind_text(statement, 4, object_string_any(storage, "updated_at", "updatedAt", NULL, created_at));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && migrations != NULL && index < json_array_get_length(migrations); index++) {
    JsonObject *migration = NULL;
    ok = json_array_object_at(migrations, index, &migration);
    const gchar *migration_id = ok ? object_string_any(migration, "migration_id", "migrationId", NULL, NULL) : NULL;
    const gchar *app_id = ok ? object_string_any(migration, "app_id", "appId", NULL, NULL) : NULL;
    if (migration_id == NULL || migration_id[0] == '\0' || app_id == NULL || app_id[0] == '\0') {
      ok = FALSE;
      break;
    }
    g_autofree gchar *migration_json = object_json_text_any(migration, "migration_json", "migrationJson", "migration", "{}");
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, migration_id);
      bind_text(statement, 2, app_id);
      sqlite3_bind_int64(statement, 3, object_int_any(migration, "from_data_version", "fromDataVersion", NULL, 1));
      sqlite3_bind_int64(statement, 4, object_int_any(migration, "to_data_version", "toDataVersion", NULL, 1));
      bind_text(statement, 5, migration_json);
      bind_text(statement, 6, object_string_any(migration, "content_hash", "contentHash", NULL, ""));
      bind_text(statement, 7, object_string_any(migration, "created_at", "createdAt", NULL, created_at));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  for (guint index = 0; ok && reports != NULL && index < json_array_get_length(reports); index++) {
    JsonObject *report = NULL;
    ok = json_array_object_at(reports, index, &report);
    const gchar *report_id = ok ? object_string_any(report, "report_id", "reportId", NULL, NULL) : NULL;
    const gchar *app_id = ok ? object_string_any(report, "app_id", "appId", NULL, NULL) : NULL;
    if (report_id == NULL || report_id[0] == '\0' || app_id == NULL || app_id[0] == '\0') {
      ok = FALSE;
      break;
    }
    g_autofree gchar *validation_json = object_json_text_any(report, "validation_json", "validationJson", "validation", NULL);
    g_autofree gchar *security_json = object_json_text_any(report, "security_json", "securityJson", "security", NULL);
    g_autofree gchar *permissions_json = object_json_text_any(report, "permissions_json", "permissionsJson", "permissions", NULL);
    g_autofree gchar *compatibility_json = object_json_text_any(report, "compatibility_json", "compatibilityJson", "compatibility", NULL);
    g_autofree gchar *smoke_test_json = object_json_text_any(report, "smoke_test_json", "smokeTestJson", "smokeTest", NULL);
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_text(statement, 1, report_id);
      bind_text(statement, 2, app_id);
      bind_nullable_text(statement, 3, object_string_any(report, "install_id", "installId", NULL, NULL));
      bind_text(statement, 4, object_string(report, "status", "accepted"));
      bind_nullable_text(statement, 5, validation_json);
      bind_nullable_text(statement, 6, security_json);
      bind_nullable_text(statement, 7, permissions_json);
      bind_nullable_text(statement, 8, compatibility_json);
      bind_nullable_text(statement, 9, smoke_test_json);
      bind_nullable_text(statement, 10, object_string_any(report, "content_hash", "contentHash", NULL, NULL));
      bind_text(statement, 11, object_string_any(report, "created_at", "createdAt", NULL, created_at));
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
  }

  JsonObject *source = object_object(document, "source");
  const gchar *source_platform = object_string(source, "platform", "unknown");
  const gchar *content_hash = object_string(document, "contentHash", NULL);
  g_autofree gchar *document_text = json_node_to_text(document_node);
  g_autofree gchar *computed_content_hash = NULL;
  if (content_hash == NULL || content_hash[0] == '\0') {
    g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, document_text, -1);
    computed_content_hash = g_strdup_printf("sha256:%s", hash);
    content_hash = computed_content_hash;
  }
  g_autofree gchar *import_id = make_id("import");
  sqlite3_stmt *statement = NULL;
  ok = ok && sqlite3_prepare_v2(
                 db,
                 "INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at) "
                 "VALUES (?, 'import', ?, ?, ?, ?, ?, ?)",
                 -1,
                 &statement,
                 NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, import_id);
    bind_text(statement, 2, source_platform);
    bind_text(statement, 3, object_string(document, "runtimeVersion", "0.4.0"));
    bind_text(statement, 4, document_text);
    bind_text(statement, 5, content_hash);
    bind_text(statement, 6, created_at);
    bind_text(statement, 7, created_at);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);

  if (!ok || sqlite3_exec(db, "COMMIT", NULL, NULL, &sql_error) != SQLITE_OK) {
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    sqlite3_free(sql_error);
    platform_database_close(db);
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Backup import could not be completed");
    return NULL;
  }
  sqlite3_free(sql_error);
  platform_database_close(db);

  return g_strdup_printf(
      "{\"ok\":true,\"apps\":%u,\"appVersions\":%u,\"appStorage\":%u}",
      json_array_get_length(apps),
      json_array_get_length(versions),
      json_array_get_length(storage_rows));
}

static gchar *db_query_rows_json(DevControlPlane *plane, const gchar *tool, const gchar *app_id, GError **error) {
  const SafeDbTable *spec = safe_db_table_for_tool(tool);
  if (spec == NULL) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Unsupported DB inspection command");
    return NULL;
  }

  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *rows = safe_table_rows_json(db, spec, app_id);
  platform_database_close(db);
  return g_strdup_printf("{\"rows\":%s}", rows);
}

static gboolean storage_command_args(JsonObject *body, const gchar *tool, gboolean require_value, JsonObject **args_out, const gchar **app_id_out, const gchar **key_out, GError **error) {
  JsonObject *args = object_object(body, "args");
  if (args == NULL) {
    g_set_error(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "%s requires args object", tool);
    return FALSE;
  }

  const gchar *app_id = object_string(args, "appId", NULL);
  const gchar *key = object_string(args, "key", NULL);
  if (app_id == NULL || app_id[0] == '\0' || key == NULL || key[0] == '\0' || (require_value && !json_object_has_member(args, "value"))) {
    if (g_strcmp0(tool, "runtime.storage_get") == 0) {
      g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.storage_get requires appId and key");
    } else if (g_strcmp0(tool, "runtime.storage_set") == 0) {
      g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.storage_set requires appId, key, and value");
    } else {
      g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.assert_storage requires appId, key, and value");
    }
    return FALSE;
  }
  if (!valid_generated_app_id(app_id)) {
    g_set_error(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "%s appId is not a valid generated app id", tool);
    return FALSE;
  }

  *args_out = args;
  *app_id_out = app_id;
  *key_out = key;
  return TRUE;
}

static gchar *runtime_storage_bridge_json(DevControlPlane *plane, const gchar *control_session_id, const gchar *app_id, const gchar *storage_method, JsonObject *args, const gchar *default_request_id) {
  const gchar *key = object_string(args, "key", "");
  const gchar *request_id = object_string(args, "id", default_request_id);
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "key");
  json_builder_add_string_value(builder, key);
  if (g_strcmp0(storage_method, "storage.get") == 0) {
    json_builder_set_member_name(builder, "defaultValue");
    JsonNode *default_value = json_object_get_member(args, "defaultValue");
    default_value == NULL ? json_builder_add_null_value(builder) : json_builder_add_value(builder, json_node_copy(default_value));
  } else {
    json_builder_set_member_name(builder, "value");
    JsonNode *value = json_object_get_member(args, "value");
    value == NULL ? json_builder_add_null_value(builder) : json_builder_add_value(builder, json_node_copy(value));
  }
  json_builder_end_object(builder);
  JsonNode *params = json_builder_get_root(builder);
  g_autofree gchar *bridge_body = bridge_call_request_json(request_id, storage_method, params);
  json_node_unref(params);
  g_object_unref(builder);

  AppSandboxContext context = app_sandbox_context_for_app(app_id, control_session_id);
  return web_bridge_handle_json(plane->bridge, bridge_body, context);
}

static gchar *stored_storage_value_json(sqlite3 *db, const gchar *app_id, const gchar *key, gboolean *found, GError **error) {
  *found = FALSE;
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, NULL) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not read app storage: %s", sqlite3_errmsg(db));
    return NULL;
  }
  bind_text(statement, 1, app_id);
  bind_text(statement, 2, key);
  gchar *value_json = NULL;
  gint step = sqlite3_step(statement);
  if (step == SQLITE_ROW) {
    const gchar *text = (const gchar *)sqlite3_column_text(statement, 0);
    value_json = g_strdup(text == NULL ? "null" : text);
    *found = TRUE;
  } else if (step != SQLITE_DONE) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not read app storage: %s", sqlite3_errmsg(db));
  }
  sqlite3_finalize(statement);
  return value_json;
}

static gchar *runtime_assert_storage_json(DevControlPlane *plane, const gchar *app_id, const gchar *key, JsonNode *expected, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  gboolean found = FALSE;
  g_autofree gchar *actual_json = stored_storage_value_json(db, app_id, key, &found, error);
  platform_database_close(db);
  if (!found) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Expected storage key was not found");
    return NULL;
  }
  if (actual_json == NULL) {
    return NULL;
  }

  JsonParser *actual_parser = json_parser_new();
  if (!json_parser_load_from_data(actual_parser, actual_json, -1, NULL)) {
    g_object_unref(actual_parser);
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Stored value was not valid JSON");
    return NULL;
  }
  g_autofree gchar *actual_canonical = json_node_to_text(json_parser_get_root(actual_parser));
  g_autofree gchar *expected_canonical = json_node_to_text(expected);
  if (g_strcmp0(actual_canonical, expected_canonical) != 0) {
    g_object_unref(actual_parser);
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Storage value did not match expected value");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "key");
  json_builder_add_string_value(builder, key);
  json_builder_set_member_name(builder, "value");
  json_builder_add_value(builder, json_node_copy(json_parser_get_root(actual_parser)));
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  g_object_unref(actual_parser);
  return text;
}

static gint64 count_rows_for_app(sqlite3 *db, const gchar *table, const gchar *app_id) {
  g_autofree gchar *sql = g_strdup_printf("SELECT COUNT(*) FROM %s WHERE app_id = ?", table);
  sqlite3_stmt *statement = NULL;
  gint64 count = 0;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    bind_text(statement, 1, app_id);
    if (sqlite3_step(statement) == SQLITE_ROW) {
      count = sqlite3_column_int64(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return count;
}

static gboolean delete_rows_for_app(sqlite3 *db, const gchar *table, const gchar *app_id, GError **error) {
  g_autofree gchar *sql = g_strdup_printf("DELETE FROM %s WHERE app_id = ?", table);
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not clear %s: %s", table, sqlite3_errmsg(db));
    return FALSE;
  }
  bind_text(statement, 1, app_id);
  gboolean ok = sqlite3_step(statement) == SQLITE_DONE;
  if (!ok) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not clear %s: %s", table, sqlite3_errmsg(db));
  }
  sqlite3_finalize(statement);
  return ok;
}

static gint64 delete_runtime_log_rows(sqlite3 *db, const gchar *table, const gchar *app_id, gboolean *ok) {
  g_autofree gchar *sql = app_id == NULL || app_id[0] == '\0'
      ? g_strdup_printf("DELETE FROM %s", table)
      : g_strdup_printf("DELETE FROM %s WHERE app_id = ?", table);
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK) {
    *ok = FALSE;
    return 0;
  }
  if (app_id != NULL && app_id[0] != '\0') {
    bind_text(statement, 1, app_id);
  }
  if (sqlite3_step(statement) != SQLITE_DONE) {
    *ok = FALSE;
    sqlite3_finalize(statement);
    return 0;
  }
  gint64 changes = sqlite3_changes(db);
  sqlite3_finalize(statement);
  return changes;
}

static gchar *clear_runtime_logs_json(DevControlPlane *plane, const gchar *app_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  gboolean ok = TRUE;
  gint64 bridge_calls = delete_runtime_log_rows(db, "bridge_calls", app_id, &ok);
  gint64 core_actions = delete_runtime_log_rows(db, "core_actions", app_id, &ok);
  gint64 core_events = delete_runtime_log_rows(db, "core_events", app_id, &ok);
  platform_database_close(db);
  if (!ok) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not clear runtime logs");
    return NULL;
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "appId");
  app_id == NULL || app_id[0] == '\0' ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "bridgeCallsCleared");
  json_builder_add_int_value(builder, bridge_calls);
  json_builder_set_member_name(builder, "coreActionsCleared");
  json_builder_add_int_value(builder, core_actions);
  json_builder_set_member_name(builder, "coreEventsCleared");
  json_builder_add_int_value(builder, core_events);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gchar *control_session_runtime_session_id(sqlite3 *db, const gchar *control_session_id) {
  sqlite3_stmt *statement = NULL;
  gchar *runtime_session_id = NULL;
  if (sqlite3_prepare_v2(db, "SELECT runtime_session_id FROM control_sessions WHERE control_session_id = ?", -1, &statement, NULL) == SQLITE_OK) {
    bind_text(statement, 1, control_session_id);
    if (sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_text(statement, 0) != NULL) {
      runtime_session_id = g_strdup((const gchar *)sqlite3_column_text(statement, 0));
    }
  }
  sqlite3_finalize(statement);
  return runtime_session_id;
}

static gchar *platform_create_snapshot_json(DevControlPlane *plane, const gchar *control_session_id, const gchar *session_id_arg, const gchar *app_id, const gchar *type, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *derived_runtime_session_id = control_session_runtime_session_id(db, control_session_id);
  const gchar *runtime_session_id = session_id_arg != NULL && session_id_arg[0] != '\0' ? session_id_arg : derived_runtime_session_id;
  g_autofree gchar *snapshot_id = make_snapshot_id();
  g_autofree gchar *created_at = now_iso();
  g_autofree gchar *install_id = NULL;
  g_autofree gchar *active_version = NULL;
  gint64 data_version = 1;
  if (!active_app_snapshot_metadata(db, app_id, &install_id, &active_version, &data_version)) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not read active app metadata: %s", sqlite3_errmsg(db));
    platform_database_close(db);
    return NULL;
  }
  g_autofree gchar *storage_rows = snapshot_storage_rows_json(db, app_id);

  JsonBuilder *snapshot_builder = json_builder_new();
  json_builder_begin_object(snapshot_builder);
  json_builder_set_member_name(snapshot_builder, "appId");
  json_builder_add_string_value(snapshot_builder, app_id);
  json_builder_set_member_name(snapshot_builder, "activeInstallId");
  install_id == NULL ? json_builder_add_null_value(snapshot_builder) : json_builder_add_string_value(snapshot_builder, install_id);
  json_builder_set_member_name(snapshot_builder, "activeVersion");
  active_version == NULL ? json_builder_add_null_value(snapshot_builder) : json_builder_add_string_value(snapshot_builder, active_version);
  json_builder_set_member_name(snapshot_builder, "dataVersion");
  json_builder_add_int_value(snapshot_builder, data_version);
  json_builder_set_member_name(snapshot_builder, "storage");
  json_builder_add_json_text_or_null(snapshot_builder, storage_rows);
  json_builder_set_member_name(snapshot_builder, "createdAt");
  json_builder_add_string_value(snapshot_builder, created_at);
  json_builder_end_object(snapshot_builder);
  g_autofree gchar *snapshot_json = json_builder_to_text(snapshot_builder);
  g_object_unref(snapshot_builder);
  g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, snapshot_json, -1);
  g_autofree gchar *content_hash = g_strdup_printf("sha256:%s", hash);

  sqlite3_stmt *statement = NULL;
  gboolean ok = sqlite3_prepare_v2(
      db,
      "INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) "
      "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      -1,
      &statement,
      NULL) == SQLITE_OK;
  if (ok) {
    bind_text(statement, 1, snapshot_id);
    bind_nullable_text(statement, 2, runtime_session_id);
    bind_text(statement, 3, app_id);
    bind_nullable_text(statement, 4, install_id);
    bind_text(statement, 5, type == NULL || type[0] == '\0' ? "manual" : type);
    bind_text(statement, 6, snapshot_json);
    bind_text(statement, 7, content_hash);
    bind_text(statement, 8, created_at);
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  if (!ok) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not create runtime snapshot: %s", sqlite3_errmsg(db));
    platform_database_close(db);
    return NULL;
  }
  platform_database_close(db);

  JsonParser *snapshot_parser = json_parser_new();
  if (!json_parser_load_from_data(snapshot_parser, snapshot_json, -1, NULL)) {
    g_object_unref(snapshot_parser);
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Created snapshot was not valid JSON");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "snapshotId");
  json_builder_add_string_value(builder, snapshot_id);
  json_builder_set_member_name(builder, "contentHash");
  json_builder_add_string_value(builder, content_hash);
  json_builder_set_member_name(builder, "snapshot");
  json_builder_add_value(builder, json_node_copy(json_parser_get_root(snapshot_parser)));
  json_builder_set_member_name(builder, "appId");
  json_builder_add_string_value(builder, app_id);
  json_builder_set_member_name(builder, "activeInstallId");
  install_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, install_id);
  json_builder_set_member_name(builder, "activeVersion");
  active_version == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, active_version);
  json_builder_set_member_name(builder, "dataVersion");
  json_builder_add_int_value(builder, data_version);
  json_builder_set_member_name(builder, "storage");
  json_builder_add_json_text_or_null(builder, storage_rows);
  json_builder_set_member_name(builder, "createdAt");
  json_builder_add_string_value(builder, created_at);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  g_object_unref(snapshot_parser);
  return text;
}

static gchar *runtime_snapshot_json_by_id(sqlite3 *db, const gchar *snapshot_id, gchar **content_hash, GError **error) {
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, "SELECT snapshot_json, content_hash FROM runtime_snapshots WHERE snapshot_id = ?", -1, &statement, NULL) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not read runtime snapshot: %s", sqlite3_errmsg(db));
    return NULL;
  }
  bind_text(statement, 1, snapshot_id);
  gchar *snapshot_json = NULL;
  if (sqlite3_step(statement) == SQLITE_ROW) {
    snapshot_json = g_strdup((const gchar *)sqlite3_column_text(statement, 0));
    if (content_hash != NULL && sqlite3_column_text(statement, 1) != NULL) {
      *content_hash = g_strdup((const gchar *)sqlite3_column_text(statement, 1));
    }
  }
  sqlite3_finalize(statement);
  if (snapshot_json == NULL) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_NOENT, "Runtime snapshot not found: %s", snapshot_id);
  }
  return snapshot_json;
}

static gchar *runtime_snapshot_app_id(DevControlPlane *plane, const gchar *snapshot_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  g_autofree gchar *snapshot_json = runtime_snapshot_json_by_id(db, snapshot_id, NULL, error);
  platform_database_close(db);
  if (snapshot_json == NULL) {
    return NULL;
  }
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, snapshot_json, -1, NULL) || !JSON_NODE_HOLDS_OBJECT(json_parser_get_root(parser))) {
    g_object_unref(parser);
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Runtime snapshot JSON is invalid");
    return NULL;
  }
  gchar *app_id = g_strdup(object_string(json_node_get_object(json_parser_get_root(parser)), "appId", NULL));
  g_object_unref(parser);
  return app_id;
}

static gboolean insert_storage_snapshot_row(sqlite3 *db, JsonObject *row, const gchar *fallback_app_id, const gchar *updated_at, GError **error) {
  const gchar *app_id = object_string(row, "app_id", fallback_app_id);
  const gchar *key = object_string(row, "key", NULL);
  const gchar *value_json = object_string(row, "value_json", "null");
  if (app_id == NULL || app_id[0] == '\0' || key == NULL || key[0] == '\0') {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Snapshot storage row requires app_id and key");
    return FALSE;
  }
  if (fallback_app_id != NULL && fallback_app_id[0] != '\0' && g_strcmp0(app_id, fallback_app_id) != 0) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Snapshot storage row app_id does not match snapshot appId");
    return FALSE;
  }
  g_autofree gchar *expected_prefix = g_strdup_printf("%s:", app_id);
  if (!g_str_has_prefix(key, expected_prefix)) {
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Snapshot storage key is outside app storage prefix");
    return FALSE;
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)", -1, &statement, NULL) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not restore storage row: %s", sqlite3_errmsg(db));
    return FALSE;
  }
  bind_text(statement, 1, app_id);
  bind_text(statement, 2, key);
  bind_text(statement, 3, value_json);
  bind_text(statement, 4, updated_at);
  gboolean ok = sqlite3_step(statement) == SQLITE_DONE;
  if (!ok) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not restore storage row: %s", sqlite3_errmsg(db));
  }
  sqlite3_finalize(statement);
  return ok;
}

static gchar *platform_restore_snapshot_json(DevControlPlane *plane, const gchar *snapshot_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *snapshot_json = runtime_snapshot_json_by_id(db, snapshot_id, NULL, error);
  if (snapshot_json == NULL) {
    platform_database_close(db);
    return NULL;
  }
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, snapshot_json, -1, NULL) || !JSON_NODE_HOLDS_OBJECT(json_parser_get_root(parser))) {
    g_object_unref(parser);
    platform_database_close(db);
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Runtime snapshot JSON is invalid");
    return NULL;
  }
  JsonObject *snapshot = json_node_get_object(json_parser_get_root(parser));
  g_autofree gchar *snapshot_app_id = g_strdup(object_string(snapshot, "appId", NULL));
  JsonArray *storage = json_object_has_member(snapshot, "storage") && JSON_NODE_HOLDS_ARRAY(json_object_get_member(snapshot, "storage"))
      ? json_object_get_array_member(snapshot, "storage")
      : NULL;
  g_autofree gchar *updated_at = now_iso();

  char *sql_error = NULL;
  if (sqlite3_exec(db, "BEGIN IMMEDIATE", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not begin snapshot restore: %s", sql_error == NULL ? sqlite3_errmsg(db) : sql_error);
    sqlite3_free(sql_error);
    g_object_unref(parser);
    platform_database_close(db);
    return NULL;
  }

  gboolean ok = TRUE;
  if (snapshot_app_id != NULL && snapshot_app_id[0] != '\0') {
    ok = delete_rows_for_app(db, "app_storage", snapshot_app_id, error);
  }
  guint restored = 0;
  if (ok && storage != NULL) {
    guint length = json_array_get_length(storage);
    for (guint index = 0; index < length; index++) {
      JsonNode *item = json_array_get_element(storage, index);
      if (!JSON_NODE_HOLDS_OBJECT(item) ||
          !insert_storage_snapshot_row(db, json_node_get_object(item), snapshot_app_id, updated_at, error)) {
        ok = FALSE;
        break;
      }
      restored++;
    }
  }
  if (ok && snapshot_app_id != NULL && snapshot_app_id[0] != '\0' && json_object_has_member(snapshot, "activeInstallId")) {
    sqlite3_stmt *statement = NULL;
    ok = sqlite3_prepare_v2(
        db,
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        -1,
        &statement,
        NULL) == SQLITE_OK;
    if (ok) {
      bind_nullable_text(statement, 1, object_string(snapshot, "activeInstallId", NULL));
      bind_nullable_text(statement, 2, object_string(snapshot, "activeVersion", NULL));
      sqlite3_bind_int64(statement, 3, json_object_get_int_member_with_default(snapshot, "dataVersion", 1));
      bind_text(statement, 4, updated_at);
      bind_text(statement, 5, snapshot_app_id);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    if (!ok && error != NULL && *error == NULL) {
      g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not restore active app pointer: %s", sqlite3_errmsg(db));
    }
  }
  if (!ok) {
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    g_object_unref(parser);
    platform_database_close(db);
    return NULL;
  }
  if (sqlite3_exec(db, "COMMIT", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not commit snapshot restore: %s", sql_error == NULL ? sqlite3_errmsg(db) : sql_error);
    sqlite3_free(sql_error);
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    g_object_unref(parser);
    platform_database_close(db);
    return NULL;
  }
  g_object_unref(parser);
  platform_database_close(db);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "snapshotId");
  json_builder_add_string_value(builder, snapshot_id);
  json_builder_set_member_name(builder, "appId");
  snapshot_app_id == NULL || snapshot_app_id[0] == '\0' ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, snapshot_app_id);
  json_builder_set_member_name(builder, "restoredStorageKeys");
  json_builder_add_int_value(builder, restored);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gboolean snapshot_compare_skip_member(const gchar *member) {
  return g_strcmp0(member, "createdAt") == 0 ||
         g_strcmp0(member, "snapshotId") == 0 ||
         g_strcmp0(member, "updated_at") == 0 ||
         g_strcmp0(member, "updatedAt") == 0;
}

static const gchar *snapshot_storage_row_string(JsonObject *row, const gchar *snake, const gchar *camel) {
  const gchar *value = object_string(row, snake, NULL);
  return value != NULL ? value : object_string(row, camel, "");
}

static gint snapshot_storage_node_compare(gconstpointer left, gconstpointer right) {
  JsonNode *left_node = (JsonNode *)left;
  JsonNode *right_node = (JsonNode *)right;
  if (!JSON_NODE_HOLDS_OBJECT(left_node) || !JSON_NODE_HOLDS_OBJECT(right_node)) {
    return 0;
  }
  JsonObject *left_object = json_node_get_object(left_node);
  JsonObject *right_object = json_node_get_object(right_node);
  const gchar *left_app = snapshot_storage_row_string(left_object, "app_id", "appId");
  const gchar *right_app = snapshot_storage_row_string(right_object, "app_id", "appId");
  gint app_compare = g_strcmp0(left_app, right_app);
  if (app_compare != 0) {
    return app_compare;
  }
  return g_strcmp0(object_string(left_object, "key", ""), object_string(right_object, "key", ""));
}

static void append_comparable_snapshot_value(JsonBuilder *builder, JsonNode *node);

static void append_sorted_storage_array(JsonBuilder *builder, JsonArray *array) {
  GList *items = NULL;
  guint length = json_array_get_length(array);
  for (guint index = 0; index < length; index++) {
    items = g_list_prepend(items, json_array_get_element(array, index));
  }
  items = g_list_sort(items, snapshot_storage_node_compare);
  json_builder_begin_array(builder);
  for (GList *item = items; item != NULL; item = item->next) {
    append_comparable_snapshot_value(builder, (JsonNode *)item->data);
  }
  json_builder_end_array(builder);
  g_list_free(items);
}

static void append_comparable_snapshot_value(JsonBuilder *builder, JsonNode *node) {
  if (node == NULL || JSON_NODE_HOLDS_NULL(node)) {
    json_builder_add_null_value(builder);
    return;
  }
  if (JSON_NODE_HOLDS_OBJECT(node)) {
    JsonObject *object = json_node_get_object(node);
    GList *members = json_object_get_members(object);
    members = g_list_sort(members, (GCompareFunc)g_strcmp0);
    json_builder_begin_object(builder);
    for (GList *item = members; item != NULL; item = item->next) {
      const gchar *member = item->data;
      if (snapshot_compare_skip_member(member)) {
        continue;
      }
      json_builder_set_member_name(builder, member);
      JsonNode *child = json_object_get_member(object, member);
      if (g_strcmp0(member, "storage") == 0 && JSON_NODE_HOLDS_ARRAY(child)) {
        append_sorted_storage_array(builder, json_node_get_array(child));
      } else {
        append_comparable_snapshot_value(builder, child);
      }
    }
    json_builder_end_object(builder);
    g_list_free(members);
    return;
  }
  if (JSON_NODE_HOLDS_ARRAY(node)) {
    JsonArray *array = json_node_get_array(node);
    json_builder_begin_array(builder);
    guint length = json_array_get_length(array);
    for (guint index = 0; index < length; index++) {
      append_comparable_snapshot_value(builder, json_array_get_element(array, index));
    }
    json_builder_end_array(builder);
    return;
  }
  json_builder_add_value(builder, json_node_copy(node));
}

static gchar *comparable_snapshot_json(const gchar *snapshot_json, GError **error) {
  JsonParser *parser = json_parser_new();
  if (!json_parser_load_from_data(parser, snapshot_json, -1, NULL)) {
    g_object_unref(parser);
    g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "Snapshot value is not valid JSON");
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  append_comparable_snapshot_value(builder, json_parser_get_root(parser));
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  g_object_unref(parser);
  return text;
}

static gchar *snapshot_arg_json(DevControlPlane *plane, JsonObject *args, const gchar *value_member, const gchar *id_member, GError **error) {
  if (json_object_has_member(args, id_member)) {
    const gchar *snapshot_id = object_string(args, id_member, NULL);
    if (snapshot_id == NULL || snapshot_id[0] == '\0') {
      g_set_error(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "%s must be a string", id_member);
      return NULL;
    }
    sqlite3 *db = platform_database_open(plane->database_path);
    if (db == NULL) {
      g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
      return NULL;
    }
    gchar *snapshot_json = runtime_snapshot_json_by_id(db, snapshot_id, NULL, error);
    platform_database_close(db);
    return snapshot_json;
  }
  if (json_object_has_member(args, value_member)) {
    return json_node_to_text(json_object_get_member(args, value_member));
  }
  g_set_error_literal(error, G_MARKUP_ERROR, G_MARKUP_ERROR_INVALID_CONTENT, "runtime.compare_snapshot requires left/right snapshots or snapshot ids");
  return NULL;
}

static gchar *runtime_compare_snapshot_json(DevControlPlane *plane, JsonObject *args, GError **error) {
  g_autofree gchar *left_json = snapshot_arg_json(plane, args, "left", "leftSnapshotId", error);
  if (left_json == NULL) {
    return NULL;
  }
  g_autofree gchar *right_json = snapshot_arg_json(plane, args, "right", "rightSnapshotId", error);
  if (right_json == NULL) {
    return NULL;
  }
  g_autofree gchar *left_comparable = comparable_snapshot_json(left_json, error);
  if (left_comparable == NULL) {
    return NULL;
  }
  g_autofree gchar *right_comparable = comparable_snapshot_json(right_json, error);
  if (right_comparable == NULL) {
    return NULL;
  }
  gboolean equal = g_strcmp0(left_comparable, right_comparable) == 0;
  g_autofree gchar *left_hash_raw = g_compute_checksum_for_string(G_CHECKSUM_SHA256, left_comparable, -1);
  g_autofree gchar *right_hash_raw = g_compute_checksum_for_string(G_CHECKSUM_SHA256, right_comparable, -1);
  g_autofree gchar *left_hash = g_strdup_printf("sha256:%s", left_hash_raw);
  g_autofree gchar *right_hash = g_strdup_printf("sha256:%s", right_hash_raw);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, equal);
  json_builder_set_member_name(builder, "equal");
  json_builder_add_boolean_value(builder, equal);
  json_builder_set_member_name(builder, "leftHash");
  json_builder_add_string_value(builder, left_hash);
  json_builder_set_member_name(builder, "rightHash");
  json_builder_add_string_value(builder, right_hash);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  return text;
}

static gboolean valid_snapshot_type(const gchar *type) {
  return type == NULL || type[0] == '\0' ||
         g_strcmp0(type, "bug-report") == 0 ||
         g_strcmp0(type, "pre-install") == 0 ||
         g_strcmp0(type, "pre-migration") == 0 ||
         g_strcmp0(type, "post-test") == 0 ||
         g_strcmp0(type, "golden") == 0 ||
         g_strcmp0(type, "manual") == 0 ||
         g_strcmp0(type, "debug-bundle") == 0;
}

static gchar *runtime_storage_reset_json(DevControlPlane *plane, const gchar *control_session_id, const gchar *app_id, gboolean clear_logs, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }

  g_autofree gchar *runtime_session_id = control_session_runtime_session_id(db, control_session_id);
  g_autofree gchar *snapshot_id = make_snapshot_id();
  g_autofree gchar *created_at = now_iso();
  g_autofree gchar *install_id = active_install_id(db, app_id);
  g_autofree gchar *storage_rows = snapshot_storage_rows_json(db, app_id);

  JsonBuilder *snapshot_builder = json_builder_new();
  json_builder_begin_object(snapshot_builder);
  json_builder_set_member_name(snapshot_builder, "appId");
  json_builder_add_string_value(snapshot_builder, app_id);
  json_builder_set_member_name(snapshot_builder, "activeInstallId");
  install_id == NULL ? json_builder_add_null_value(snapshot_builder) : json_builder_add_string_value(snapshot_builder, install_id);
  json_builder_set_member_name(snapshot_builder, "storage");
  json_builder_add_json_text_or_null(snapshot_builder, storage_rows);
  json_builder_set_member_name(snapshot_builder, "createdAt");
  json_builder_add_string_value(snapshot_builder, created_at);
  json_builder_end_object(snapshot_builder);
  g_autofree gchar *snapshot_json = json_builder_to_text(snapshot_builder);
  g_object_unref(snapshot_builder);
  g_autofree gchar *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, snapshot_json, -1);
  g_autofree gchar *content_hash = g_strdup_printf("sha256:%s", hash);

  char *sql_error = NULL;
  if (sqlite3_exec(db, "BEGIN IMMEDIATE", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not begin storage reset transaction: %s", sql_error == NULL ? sqlite3_errmsg(db) : sql_error);
    sqlite3_free(sql_error);
    platform_database_close(db);
    return NULL;
  }

  gboolean ok = TRUE;
  sqlite3_stmt *snapshot = NULL;
  ok = sqlite3_prepare_v2(
           db,
           "INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) "
           "VALUES (?, ?, ?, ?, 'manual', ?, ?, ?)",
           -1,
           &snapshot,
           NULL) == SQLITE_OK;
  if (ok) {
    bind_text(snapshot, 1, snapshot_id);
    if (runtime_session_id == NULL) {
      sqlite3_bind_null(snapshot, 2);
    } else {
      bind_text(snapshot, 2, runtime_session_id);
    }
    bind_text(snapshot, 3, app_id);
    if (install_id == NULL) {
      sqlite3_bind_null(snapshot, 4);
    } else {
      bind_text(snapshot, 4, install_id);
    }
    bind_text(snapshot, 5, snapshot_json);
    bind_text(snapshot, 6, content_hash);
    bind_text(snapshot, 7, created_at);
    ok = sqlite3_step(snapshot) == SQLITE_DONE;
  }
  sqlite3_finalize(snapshot);
  if (!ok) {
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not create pre-reset runtime snapshot: %s", sqlite3_errmsg(db));
    platform_database_close(db);
    return NULL;
  }

  gint64 cleared_storage_keys = count_rows_for_app(db, "app_storage", app_id);
  gint64 cleared_bridge_calls = clear_logs ? count_rows_for_app(db, "bridge_calls", app_id) : 0;
  gint64 cleared_core_events = clear_logs ? count_rows_for_app(db, "core_events", app_id) : 0;
  if (!delete_rows_for_app(db, "app_storage", app_id, error) ||
      (clear_logs && (!delete_rows_for_app(db, "bridge_calls", app_id, error) || !delete_rows_for_app(db, "core_events", app_id, error)))) {
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    platform_database_close(db);
    return NULL;
  }

  if (sqlite3_exec(db, "COMMIT", NULL, NULL, &sql_error) != SQLITE_OK) {
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not commit storage reset: %s", sql_error == NULL ? sqlite3_errmsg(db) : sql_error);
    sqlite3_free(sql_error);
    sqlite3_exec(db, "ROLLBACK", NULL, NULL, NULL);
    platform_database_close(db);
    return NULL;
  }
  platform_database_close(db);

  return g_strdup_printf(
      "{\"ok\":true,\"appId\":\"%s\",\"snapshotId\":\"%s\",\"clearedStorageKeys\":%" G_GINT64_FORMAT ",\"storageRowsDeleted\":%" G_GINT64_FORMAT ",\"clearedBridgeCalls\":%" G_GINT64_FORMAT ",\"clearedCoreEvents\":%" G_GINT64_FORMAT "}",
      app_id,
      snapshot_id,
      cleared_storage_keys,
      cleared_storage_keys,
      cleared_bridge_calls,
      cleared_core_events);
}

static gchar *session_snapshot_json(DevControlPlane *plane, const gchar *control_session_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  ControlSessionRecord record = {0};
  if (!load_control_session(db, control_session_id, &record)) {
    platform_database_close(db);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_NOENT, "Control session not found: %s", control_session_id);
    return NULL;
  }
  g_autofree gchar *capabilities = runtime_capabilities_json(plane, record.app_id);
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "controlSessionId");
  json_builder_add_string_value(builder, record.control_session_id);
  json_builder_set_member_name(builder, "snapshot");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "target");
  json_builder_add_string_value(builder, "linux");
  json_builder_set_member_name(builder, "appId");
  record.app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.app_id);
  json_builder_set_member_name(builder, "runtimeSessionId");
  record.runtime_session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.runtime_session_id);
  json_builder_set_member_name(builder, "status");
  json_builder_add_string_value(builder, record.status);
  json_builder_set_member_name(builder, "title");
  json_builder_add_string_value(builder, record.app_id == NULL ? "Linux Native Runtime" : record.app_id);
  json_builder_set_member_name(builder, "testIds");
  json_builder_begin_array(builder);
  json_builder_end_array(builder);
  json_builder_set_member_name(builder, "resourceUsage");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "appId");
  record.app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.app_id);
  json_builder_set_member_name(builder, "bridgeCalls");
  json_builder_add_int_value(builder, count_table_for_app(db, "bridge_calls", record.app_id));
  json_builder_set_member_name(builder, "coreEvents");
  json_builder_add_int_value(builder, count_table_for_app(db, "core_events", record.app_id));
  json_builder_set_member_name(builder, "storageKeys");
  json_builder_add_int_value(builder, count_table_for_app(db, "app_storage", record.app_id));
  json_builder_end_object(builder);
  json_builder_set_member_name(builder, "capabilities");
  JsonParser *cap_parser = json_parser_new();
  if (json_parser_load_from_data(cap_parser, capabilities, -1, NULL)) {
    json_builder_add_value(builder, json_node_copy(json_parser_get_root(cap_parser)));
  } else {
    json_builder_begin_object(builder);
    json_builder_end_object(builder);
  }
  g_object_unref(cap_parser);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  control_session_record_clear(&record);
  platform_database_close(db);
  return text;
}

static gchar *session_events_json(DevControlPlane *plane, const gchar *control_session_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  ControlSessionRecord record = {0};
  if (!load_control_session(db, control_session_id, &record)) {
    platform_database_close(db);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_NOENT, "Control session not found: %s", control_session_id);
    return NULL;
  }
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "controlSessionId");
  json_builder_add_string_value(builder, record.control_session_id);
  json_builder_set_member_name(builder, "runtimeSessionId");
  record.runtime_session_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.runtime_session_id);
  json_builder_set_member_name(builder, "appId");
  record.app_id == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, record.app_id);
  json_builder_set_member_name(builder, "bridgeCalls");
  append_bridge_call_rows(builder, db, record.app_id);
  json_builder_set_member_name(builder, "coreEvents");
  append_core_event_rows(builder, db, record.app_id);
  json_builder_end_object(builder);
  gchar *text = json_builder_to_text(builder);
  g_object_unref(builder);
  control_session_record_clear(&record);
  platform_database_close(db);
  return text;
}

static gchar *session_capabilities_json(DevControlPlane *plane, const gchar *control_session_id, GError **error) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, "Could not open platform database");
    return NULL;
  }
  ControlSessionRecord record = {0};
  if (!load_control_session(db, control_session_id, &record)) {
    platform_database_close(db);
    g_set_error(error, G_FILE_ERROR, G_FILE_ERROR_NOENT, "Control session not found: %s", control_session_id);
    return NULL;
  }
  gchar *text = runtime_capabilities_json(plane, record.app_id);
  control_session_record_clear(&record);
  platform_database_close(db);
  return text;
}

static gboolean control_session_allows_app(
    DevControlPlane *plane,
    const gchar *control_session_id,
    const gchar *app_id,
    gchar **error_code,
    gchar **error_message,
    guint *status) {
  sqlite3 *db = platform_database_open(plane->database_path);
  if (db == NULL) {
    *error_code = g_strdup("storage_error");
    *error_message = g_strdup("Could not open platform database");
    *status = SOUP_STATUS_INTERNAL_SERVER_ERROR;
    return FALSE;
  }

  ControlSessionRecord record = {0};
  if (!load_control_session(db, control_session_id, &record)) {
    platform_database_close(db);
    *error_code = g_strdup("not_found");
    *error_message = g_strdup("Control session not found");
    *status = SOUP_STATUS_BAD_REQUEST;
    return FALSE;
  }

  gboolean allowed = TRUE;
  if (g_strcmp0(record.status, "running") != 0) {
    *error_code = g_strdup("invalid_request");
    *error_message = g_strdup("Control session is not running");
    *status = SOUP_STATUS_BAD_REQUEST;
    allowed = FALSE;
  } else if (app_id != NULL && app_id[0] != '\0' && record.app_id != NULL && g_strcmp0(record.app_id, app_id) != 0) {
    *error_code = g_strdup("permission_denied");
    *error_message = g_strdup("Control command appId does not match the control session app");
    *status = SOUP_STATUS_BAD_REQUEST;
    allowed = FALSE;
  }

  control_session_record_clear(&record);
  platform_database_close(db);
  return allowed;
}

static gchar *session_id_from_path(const gchar *path, const gchar *suffix) {
  const gchar *normalized = g_str_has_prefix(path, "/control/sessions/") ? path + strlen("/control") : path;
  if (!g_str_has_prefix(normalized, "/sessions/")) {
    return NULL;
  }
  const gchar *start = normalized + strlen("/sessions/");
  if (suffix == NULL) {
    if (strchr(start, '/') != NULL || start[0] == '\0') {
      return NULL;
    }
    return g_uri_unescape_string(start, NULL);
  }
  if (!g_str_has_suffix(normalized, suffix)) {
    return NULL;
  }
  const gchar *end = normalized + strlen(normalized) - strlen(suffix);
  if (end <= start || *end != '/') {
    return NULL;
  }
  return g_uri_unescape_segment(start, end, NULL);
}

static void send_control_route_error(DevControlPlane *plane, SoupServerMessage *message, const gchar *audit_session_id, const gchar *tool, const gchar *method, const gchar *path, gint64 started, const gchar *code, const gchar *message_text, guint status) {
  g_autofree gchar *body = error_json(code, message_text);
  send_json(message, status, body);
  audit_control_request(plane, audit_session_id, tool, method, path, "rejected", code, NULL, body, (g_get_real_time() - started) / 1000);
}

static void send_control_route_result(DevControlPlane *plane, SoupServerMessage *message, const gchar *audit_session_id, const gchar *tool, const gchar *method, const gchar *path, gint64 started, const gchar *result_json) {
  g_autofree gchar *body = control_ok_json(result_json);
  send_json(message, SOUP_STATUS_OK, body);
  audit_control_request(plane, audit_session_id, tool, method, path, "accepted", NULL, result_json, NULL, (g_get_real_time() - started) / 1000);
}

static void health_handler(SoupServer *server, SoupServerMessage *message, const char *path, GHashTable *query, gpointer user_data) {
  (void)server;
  (void)query;
  DevControlPlane *plane = user_data;
  gint64 started = g_get_real_time();
  const gchar *method = soup_server_message_get_method(message);

  if (!authorize_request(plane, message, method, path, started)) {
    return;
  }

  if (g_strcmp0(method, "GET") != 0) {
    g_autofree gchar *body = error_json("method_not_allowed", "Only GET /health is supported");
    send_json(message, SOUP_STATUS_METHOD_NOT_ALLOWED, body);
    audit_control_request(plane, NULL, "platform.health", method, path, "rejected", "method_not_allowed", NULL, body, (g_get_real_time() - started) / 1000);
    return;
  }

  g_autofree gchar *body = health_result_json(plane);
  send_json(message, SOUP_STATUS_OK, body);
  audit_control_request(plane, NULL, "platform.health", method, path, "accepted", NULL, body, NULL, (g_get_real_time() - started) / 1000);
}

static gboolean is_sessions_collection_path(const gchar *path) {
  return g_strcmp0(path, "/sessions") == 0 || g_strcmp0(path, "/control/sessions") == 0;
}

static gboolean is_sessions_route_path(const gchar *path) {
  return is_sessions_collection_path(path) ||
         g_str_has_prefix(path, "/sessions/") ||
         g_str_has_prefix(path, "/control/sessions/");
}

static gboolean session_route_parse_body(SoupServerMessage *message, JsonParser **parser, JsonObject **body, GError **error) {
  *parser = NULL;
  *body = parse_request_object(message, parser, error);
  return *body != NULL;
}

static void session_create_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started) {
  if (g_strcmp0(method, "POST") != 0) {
    send_control_route_error(plane, message, NULL, "control.sessions.create", method, path, started, "not_found", "Control session route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }

  JsonParser *parser = NULL;
  JsonObject *body = NULL;
  GError *error = NULL;
  if (!session_route_parse_body(message, &parser, &body, &error)) {
    send_control_route_error(plane, message, NULL, "control.sessions.create", method, path, started, "invalid_request", error != NULL ? error->message : "Control session body must be JSON", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }

  g_autofree gchar *result = create_control_session(plane, body, &error);
  g_object_unref(parser);
  if (result == NULL) {
    send_control_route_error(plane, message, NULL, "control.sessions.create", method, path, started, "invalid_request", error != NULL ? error->message : "Could not create control session", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, NULL, "control.sessions.create", method, path, started, result);
}

static void session_item_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started, const gchar *control_session_id) {
  if (g_strcmp0(method, "DELETE") != 0) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.end", method, path, started, "not_found", "Control session route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }

  GError *error = NULL;
  g_autofree gchar *result = end_control_session(plane, control_session_id, &error);
  if (result == NULL) {
    send_control_route_error(plane, message, NULL, "control.sessions.end", method, path, started, "not_found", error != NULL ? error->message : "Control session not found", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, control_session_id, "control.sessions.end", method, path, started, result);
}

static void session_snapshot_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started, const gchar *control_session_id) {
  if (g_strcmp0(method, "GET") != 0) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.snapshot", method, path, started, "not_found", "Control session snapshot route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }
  GError *error = NULL;
  g_autofree gchar *result = session_snapshot_json(plane, control_session_id, &error);
  if (result == NULL) {
    send_control_route_error(plane, message, NULL, "control.sessions.snapshot", method, path, started, "not_found", error != NULL ? error->message : "Control session not found", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, control_session_id, "control.sessions.snapshot", method, path, started, result);
}

static void session_events_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started, const gchar *control_session_id) {
  if (g_strcmp0(method, "GET") != 0) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.events", method, path, started, "not_found", "Control session events route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }
  GError *error = NULL;
  g_autofree gchar *result = session_events_json(plane, control_session_id, &error);
  if (result == NULL) {
    send_control_route_error(plane, message, NULL, "control.sessions.events", method, path, started, "not_found", error != NULL ? error->message : "Control session not found", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, control_session_id, "control.sessions.events", method, path, started, result);
}

static void session_capabilities_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started, const gchar *control_session_id) {
  if (g_strcmp0(method, "GET") != 0) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.capabilities", method, path, started, "not_found", "Control session capabilities route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }
  GError *error = NULL;
  g_autofree gchar *result = session_capabilities_json(plane, control_session_id, &error);
  if (result == NULL) {
    send_control_route_error(plane, message, NULL, "control.sessions.capabilities", method, path, started, "not_found", error != NULL ? error->message : "Control session not found", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, control_session_id, "control.sessions.capabilities", method, path, started, result);
}

static void session_command_handler(DevControlPlane *plane, SoupServerMessage *message, const gchar *method, const gchar *path, gint64 started, const gchar *control_session_id) {
  if (g_strcmp0(method, "POST") != 0) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.command", method, path, started, "not_found", "Control session command route was not found", SOUP_STATUS_NOT_FOUND);
    return;
  }

  JsonParser *parser = NULL;
  JsonObject *body = NULL;
  GError *error = NULL;
  if (!session_route_parse_body(message, &parser, &body, &error)) {
    send_control_route_error(plane, message, control_session_id, "control.sessions.command", method, path, started, "invalid_request", error != NULL ? error->message : "Control command body must be JSON", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }

  g_autofree gchar *tool = g_strdup(object_string(body, "tool", NULL));
  if (tool == NULL || tool[0] == '\0') {
    g_object_unref(parser);
    send_control_route_error(plane, message, control_session_id, "control.sessions.command", method, path, started, "invalid_request", "Control command requires tool", SOUP_STATUS_BAD_REQUEST);
    return;
  }

  g_autofree gchar *result = NULL;
  if (g_strcmp0(tool, "platform.health") == 0) {
    result = health_result_json(plane);
  } else if (g_strcmp0(tool, "platform.list_targets") == 0) {
    result = platform_list_targets_json(plane);
  } else if (g_strcmp0(tool, "platform.list_webapps") == 0) {
    JsonObject *args = json_object_has_member(body, "args") ? object_object(body, "args") : NULL;
    if (json_object_has_member(body, "args") && args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.list_webapps args must be an object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    result = platform_list_webapps_json(plane, object_boolean_true(args, "includeUninstalled"), &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not list webapps", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.capabilities") == 0) {
    result = session_capabilities_json(plane, control_session_id, &error);
  } else if (g_strcmp0(tool, "runtime.resource_usage") == 0 ||
             g_strcmp0(tool, "runtime.event_log") == 0 ||
             g_strcmp0(tool, "runtime.console_logs") == 0) {
    JsonObject *args = NULL;
    const gchar *app_id = NULL;
    if (json_object_has_member(body, "args")) {
      args = object_object(body, "args");
      if (args == NULL) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime inspection command requires args object", SOUP_STATUS_BAD_REQUEST);
        return;
      }
      if (json_object_has_member(args, "appId")) {
        JsonNode *app_id_node = json_object_get_member(args, "appId");
        if (app_id_node == NULL || !JSON_NODE_HOLDS_VALUE(app_id_node) || json_node_get_value_type(app_id_node) != G_TYPE_STRING) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime inspection appId must be a string", SOUP_STATUS_BAD_REQUEST);
          return;
        }
        app_id = json_node_get_string(app_id_node);
        if (app_id != NULL && app_id[0] != '\0' && !valid_generated_app_id(app_id)) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime inspection appId is not a valid generated app id", SOUP_STATUS_BAD_REQUEST);
          return;
        }
      }
    }
    if (g_strcmp0(tool, "runtime.resource_usage") == 0 && (app_id == NULL || app_id[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.resource_usage requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    if (g_strcmp0(tool, "runtime.resource_usage") == 0) {
      result = runtime_resource_usage_json(plane, app_id, &error);
    } else if (g_strcmp0(tool, "runtime.event_log") == 0) {
      result = runtime_event_log_json(plane, app_id, &error);
    } else {
      result = runtime_console_logs_json(plane, app_id, &error);
    }
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not read runtime inspection data", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.bridge_calls") == 0 ||
             g_strcmp0(tool, "runtime.clear_logs") == 0 ||
             g_strcmp0(tool, "runtime.notification_capture") == 0 ||
             g_strcmp0(tool, "runtime.assert_no_console_errors") == 0) {
    JsonObject *args = NULL;
    const gchar *app_id = NULL;
    if (json_object_has_member(body, "args")) {
      args = object_object(body, "args");
      if (args == NULL) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime bridge log command requires args object", SOUP_STATUS_BAD_REQUEST);
        return;
      }
      if (json_object_has_member(args, "appId")) {
        JsonNode *app_id_node = json_object_get_member(args, "appId");
        if (app_id_node == NULL || !JSON_NODE_HOLDS_VALUE(app_id_node) || json_node_get_value_type(app_id_node) != G_TYPE_STRING) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime bridge log appId must be a string", SOUP_STATUS_BAD_REQUEST);
          return;
        }
        app_id = json_node_get_string(app_id_node);
        if (app_id != NULL && app_id[0] != '\0' && !valid_generated_app_id(app_id)) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime bridge log appId is not a valid generated app id", SOUP_STATUS_BAD_REQUEST);
          return;
        }
      }
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    if (g_strcmp0(tool, "runtime.bridge_calls") == 0) {
      result = runtime_bridge_calls_json(plane, app_id, &error);
    } else if (g_strcmp0(tool, "runtime.clear_logs") == 0) {
      result = clear_runtime_logs_json(plane, app_id, &error);
    } else if (g_strcmp0(tool, "runtime.notification_capture") == 0) {
      result = notification_capture_json(plane, app_id, &error);
    } else {
      result = assert_no_console_errors_json(plane, app_id, &error_code, &error_message, &error_status);
      if (result == NULL) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code != NULL ? error_code : "console_errors_found", error_message != NULL ? error_message : "Console error logs were found", error_status);
        return;
      }
    }
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not read runtime bridge log data", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.screenshot") == 0 ||
             g_strcmp0(tool, "runtime.query") == 0 ||
             g_strcmp0(tool, "runtime.click") == 0 ||
             g_strcmp0(tool, "runtime.type") == 0 ||
             g_strcmp0(tool, "runtime.set_value") == 0 ||
             g_strcmp0(tool, "runtime.press_key") == 0 ||
             g_strcmp0(tool, "runtime.drag") == 0 ||
             g_strcmp0(tool, "runtime.wait_for") == 0 ||
             g_strcmp0(tool, "runtime.timer_advance") == 0 ||
             g_strcmp0(tool, "runtime.assert_visible") == 0 ||
             g_strcmp0(tool, "runtime.assert_text") == 0) {
    JsonObject *args = NULL;
    if (json_object_has_member(body, "args")) {
      args = object_object(body, "args");
      if (args == NULL) {
        g_autofree gchar *message_text = g_strdup_printf("%s requires args object", tool);
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", message_text, SOUP_STATUS_BAD_REQUEST);
        return;
      }
    }

    const gchar *app_id = object_string(args, "appId", NULL);
    if (app_id != NULL && app_id[0] != '\0' && !valid_generated_app_id(app_id)) {
      g_autofree gchar *message_text = g_strdup_printf("%s appId is not a valid generated app id", tool);
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", message_text, SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (g_strcmp0(tool, "runtime.query") == 0 && (app_id == NULL || app_id[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.query requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (g_strcmp0(tool, "runtime.screenshot") == 0 && (app_id == NULL || app_id[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.screenshot requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if ((g_strcmp0(tool, "runtime.click") == 0 ||
         g_strcmp0(tool, "runtime.type") == 0 ||
         g_strcmp0(tool, "runtime.set_value") == 0 ||
         g_strcmp0(tool, "runtime.drag") == 0) &&
        (app_id == NULL || app_id[0] == '\0')) {
      g_autofree gchar *message_text = g_strdup_printf("%s requires appId", tool);
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", message_text, SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (g_strcmp0(tool, "runtime.assert_visible") == 0 && (app_id == NULL || app_id[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_visible requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (g_strcmp0(tool, "runtime.assert_text") == 0) {
      const gchar *text_arg = object_string(args, "text", NULL);
      if (app_id == NULL || app_id[0] == '\0' || text_arg == NULL || text_arg[0] == '\0') {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_text requires appId and text", SOUP_STATUS_BAD_REQUEST);
        return;
      }
    }
    if (g_strcmp0(tool, "runtime.wait_for") == 0) {
      const gchar *wait_kind = object_string(args, "kind", "idle");
      const gchar *bridge_method = object_string(args, "method", NULL);
      if ((g_strcmp0(wait_kind, "bridge_call") == 0 || g_strcmp0(wait_kind, "bridgeCall") == 0) &&
          (app_id == NULL || app_id[0] == '\0' || bridge_method == NULL || bridge_method[0] == '\0')) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.wait_for bridge_call requires appId and method", SOUP_STATUS_BAD_REQUEST);
        return;
      }
      if (g_strcmp0(wait_kind, "idle") != 0 &&
          g_strcmp0(wait_kind, "bridge_call") != 0 &&
          g_strcmp0(wait_kind, "bridgeCall") != 0 &&
          (app_id == NULL || app_id[0] == '\0')) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.wait_for requires appId for selector/text waits", SOUP_STATUS_BAD_REQUEST);
        return;
      }
    }

    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }

    if (g_strcmp0(tool, "runtime.screenshot") == 0) {
      result = runtime_screenshot_json(app_id, object_string(args, "label", NULL));
    } else if (g_strcmp0(tool, "runtime.query") == 0) {
      result = runtime_query_json(app_id, args);
    } else if (g_strcmp0(tool, "runtime.click") == 0 ||
               g_strcmp0(tool, "runtime.type") == 0 ||
               g_strcmp0(tool, "runtime.set_value") == 0 ||
               g_strcmp0(tool, "runtime.press_key") == 0 ||
               g_strcmp0(tool, "runtime.drag") == 0) {
      result = runtime_target_command_json(tool, args, &error_code, &error_message, &error_status);
    } else if (g_strcmp0(tool, "runtime.wait_for") == 0) {
      result = runtime_wait_for_json(plane, args, &error_code, &error_message, &error_status);
    } else if (g_strcmp0(tool, "runtime.timer_advance") == 0) {
      result = runtime_timer_advance_json(args);
    } else if (g_strcmp0(tool, "runtime.assert_visible") == 0) {
      result = runtime_assert_visible_json(app_id, args, &error_code, &error_message, &error_status);
    } else {
      result = runtime_assert_text_json(app_id, object_string(args, "text", ""), &error_code, &error_message, &error_status);
    }

    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code != NULL ? error_code : "selector.not_found", error_message != NULL ? error_message : "Runtime UI command failed", error_status);
      return;
    }
  } else if (g_strcmp0(tool, "db.export_backup") == 0 || g_strcmp0(tool, "db.export_debug_bundle") == 0) {
    JsonObject *args = NULL;
    if (json_object_has_member(body, "args")) {
      args = object_object(body, "args");
      if (args == NULL) {
        const gchar *message_text = g_strcmp0(tool, "db.export_backup") == 0 ? "db.export_backup args must be an object" : "db.export_debug_bundle args must be an object";
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", message_text, SOUP_STATUS_BAD_REQUEST);
        return;
      }
    }
    (void)args;
    result = g_strcmp0(tool, "db.export_backup") == 0 ? db_export_backup_json(plane, &error) : db_export_debug_bundle_json(plane, &error);
    if (result == NULL) {
      const gchar *message_text = g_strcmp0(tool, "db.export_backup") == 0 ? "Could not export backup" : "Could not export debug bundle";
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : message_text, SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "db.import_backup") == 0) {
    JsonObject *args = json_object_has_member(body, "args") ? object_object(body, "args") : NULL;
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "db.import_backup requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    JsonNode *backup_node = json_object_get_member(args, "backup");
    if (backup_node == NULL || !JSON_NODE_HOLDS_OBJECT(backup_node)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "db.import_backup requires backup", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    result = db_import_backup_json(plane, json_node_get_object(backup_node), backup_node, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_backup", error != NULL ? error->message : "Backup import could not be completed", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
  } else if (is_db_inspection_tool(tool)) {
    JsonObject *args = NULL;
    const gchar *app_id = NULL;
    if (json_object_has_member(body, "args")) {
      args = object_object(body, "args");
      if (args == NULL) {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "DB inspection command requires args object", SOUP_STATUS_BAD_REQUEST);
        return;
      }
      if (json_object_has_member(args, "appId")) {
        JsonNode *app_id_node = json_object_get_member(args, "appId");
        if (app_id_node == NULL || !JSON_NODE_HOLDS_VALUE(app_id_node) || json_node_get_value_type(app_id_node) != G_TYPE_STRING) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "DB inspection appId must be a string", SOUP_STATUS_BAD_REQUEST);
          return;
        }
        app_id = json_node_get_string(app_id_node);
        if (app_id != NULL && app_id[0] != '\0' && !valid_generated_app_id(app_id)) {
          g_object_unref(parser);
          send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "DB inspection appId is not a valid generated app id", SOUP_STATUS_BAD_REQUEST);
          return;
        }
      }
    }
    if (db_tool_requires_app_id(tool) && (app_id == NULL || app_id[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "DB inspection command requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = g_strcmp0(tool, "db.snapshot") == 0
        ? db_snapshot_json(plane, &error)
        : db_query_rows_json(plane, tool, app_id, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not read platform database", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "platform.create_snapshot") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.create_snapshot requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    const gchar *snapshot_type = object_string(args, "type", "manual");
    const gchar *session_id_arg = object_string(args, "sessionId", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.create_snapshot requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (json_object_has_member(args, "sessionId") && (session_id_arg == NULL || session_id_arg[0] == '\0')) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.create_snapshot sessionId must be a string", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (!valid_snapshot_type(snapshot_type)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.create_snapshot type is invalid", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = platform_create_snapshot_json(plane, control_session_id, session_id_arg, app_id, snapshot_type, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not create runtime snapshot", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "platform.restore_snapshot") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.restore_snapshot requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *snapshot_id = object_string(args, "snapshotId", NULL);
    if (snapshot_id == NULL || snapshot_id[0] == '\0') {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "platform.restore_snapshot requires snapshotId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (!object_boolean_true(args, "confirm")) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "confirmation_required", "platform.restore_snapshot requires confirm: true", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *snapshot_app_id = runtime_snapshot_app_id(plane, snapshot_id, &error);
    if (snapshot_app_id == NULL && error != NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error->domain == G_FILE_ERROR && error->code == G_FILE_ERROR_NOENT ? "snapshot_not_found" : "storage_error", error->message, SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, snapshot_app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = platform_restore_snapshot_json(plane, snapshot_id, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error != NULL && error->domain == G_FILE_ERROR && error->code == G_FILE_ERROR_NOENT ? "snapshot_not_found" : "storage_error", error != NULL ? error->message : "Could not restore runtime snapshot", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.compare_snapshot") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.compare_snapshot requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    result = runtime_compare_snapshot_json(plane, args, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error != NULL && error->domain == G_FILE_ERROR && error->code == G_FILE_ERROR_NOENT ? "snapshot_not_found" : "invalid_request", error != NULL ? error->message : "runtime.compare_snapshot requires left/right snapshots or snapshot ids", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.call_bridge") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.call_bridge requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    const gchar *bridge_method = object_string(args, "method", NULL);
    if (app_id == NULL || app_id[0] == '\0' || bridge_method == NULL || bridge_method[0] == '\0') {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.call_bridge requires appId and method", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    JsonNode *params = json_object_get_member(args, "params");
    if (params != NULL && !JSON_NODE_HOLDS_OBJECT(params)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.call_bridge params must be an object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (plane->bridge == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "platform_unsupported", "Linux dev control bridge is not available", SOUP_STATUS_SERVICE_UNAVAILABLE);
      return;
    }
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    const gchar *request_id = object_string(args, "id", "control_call_bridge");
    g_autofree gchar *bridge_body = bridge_call_request_json(request_id, bridge_method, params);
    AppSandboxContext context = app_sandbox_context_for_app(app_id, control_session_id);
    result = web_bridge_handle_json(plane->bridge, bridge_body, context);
  } else if (g_strcmp0(tool, "runtime.storage_get") == 0 ||
             g_strcmp0(tool, "runtime.storage_set") == 0) {
    JsonObject *args = NULL;
    const gchar *app_id = NULL;
    const gchar *key = NULL;
    if (!storage_command_args(body, tool, g_strcmp0(tool, "runtime.storage_set") == 0, &args, &app_id, &key, &error)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", error != NULL ? error->message : "Storage command requires args", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (plane->bridge == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "platform_unsupported", "Linux dev control bridge is not available", SOUP_STATUS_SERVICE_UNAVAILABLE);
      return;
    }
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = g_strcmp0(tool, "runtime.storage_get") == 0
        ? runtime_storage_bridge_json(plane, control_session_id, app_id, "storage.get", args, "control_storage_get")
        : runtime_storage_bridge_json(plane, control_session_id, app_id, "storage.set", args, "control_storage_set");
    (void)key;
  } else if (g_strcmp0(tool, "runtime.assert_storage") == 0) {
    JsonObject *args = NULL;
    const gchar *app_id = NULL;
    const gchar *key = NULL;
    if (!storage_command_args(body, tool, TRUE, &args, &app_id, &key, &error)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", error != NULL ? error->message : "runtime.assert_storage requires args", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = runtime_assert_storage_json(plane, app_id, key, json_object_get_member(args, "value"), &error);
    if (result == NULL) {
      const gchar *code = error != NULL && error->domain == G_FILE_ERROR ? "storage_error" : "assertion_failed";
      guint status = g_strcmp0(code, "storage_error") == 0 ? SOUP_STATUS_INTERNAL_SERVER_ERROR : SOUP_STATUS_BAD_REQUEST;
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, code, error != NULL ? error->message : "Storage assertion failed", status);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.storage_reset") == 0 ||
             g_strcmp0(tool, "platform.reset_webapp") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Storage reset command requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Storage reset command requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    if (!object_boolean_true(args, "confirm")) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "confirmation_required", "Storage reset command requires confirm: true", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = runtime_storage_reset_json(plane, control_session_id, app_id, g_strcmp0(tool, "platform.reset_webapp") == 0, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Webapp storage could not be reset", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.assert_bridge_call") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_bridge_call requires appId and method", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    const gchar *bridge_method = object_string(args, "method", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id) || bridge_method == NULL || bridge_method[0] == '\0') {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_bridge_call requires appId and method", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = assert_bridge_call_json(plane, app_id, bridge_method, &error_code, &error_message, &error_status);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code != NULL ? error_code : "assertion_failed", error_message != NULL ? error_message : "Expected bridge call was not recorded", error_status);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.network_mock_set") == 0 ||
             g_strcmp0(tool, "runtime.network_mock_reset") == 0 ||
             g_strcmp0(tool, "runtime.dialog_mock_set") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime effect mock command requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_nonempty_string(args, "appId");
    if (app_id != NULL && !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "Runtime effect mock appId is not a valid generated app id", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    if (g_strcmp0(tool, "runtime.network_mock_set") == 0) {
      result = runtime_network_mock_set_json(plane, args, &error);
    } else if (g_strcmp0(tool, "runtime.network_mock_reset") == 0) {
      result = runtime_network_mock_reset_json(plane, args, &error);
    } else {
      result = runtime_dialog_mock_set_json(plane, args, &error);
    }
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", error != NULL ? error->message : "Runtime effect mock command failed", SOUP_STATUS_BAD_REQUEST);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.core_snapshot") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.core_snapshot requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.core_snapshot requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = runtime_core_snapshot_json(plane, app_id, &error);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "storage_error", error != NULL ? error->message : "Could not read core snapshot", SOUP_STATUS_INTERNAL_SERVER_ERROR);
      g_clear_error(&error);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.replay_events") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.replay_events requires appId and events", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.replay_events requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    JsonNode *events_node = json_object_get_member(args, "events");
    if (events_node == NULL || !JSON_NODE_HOLDS_ARRAY(events_node)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.replay_events events must be an array", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = runtime_replay_events_json(app_id, json_node_get_array(events_node));
  } else if (g_strcmp0(tool, "runtime.assert_core_action") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_core_action requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    if (app_id == NULL || app_id[0] == '\0' || !valid_generated_app_id(app_id)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_core_action requires appId", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *expected_type = NULL;
    if (json_object_has_member(args, "type")) {
      JsonNode *type_node = json_object_get_member(args, "type");
      if (type_node == NULL || !JSON_NODE_HOLDS_VALUE(type_node) || json_node_get_value_type(type_node) != G_TYPE_STRING || json_node_get_string(type_node)[0] == '\0') {
        g_object_unref(parser);
        send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_core_action type must be a string", SOUP_STATUS_BAD_REQUEST);
        return;
      }
      expected_type = json_node_get_string(type_node);
    }
    JsonNode *expected_match = json_object_has_member(args, "match") ? json_object_get_member(args, "match") : NULL;
    if (expected_match != NULL && !JSON_NODE_HOLDS_OBJECT(expected_match)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.assert_core_action match must be an object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    JsonNode *expected_action = json_object_has_member(args, "action") ? json_object_get_member(args, "action") : NULL;
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    result = runtime_assert_core_action_json(plane, app_id, expected_type, expected_match, expected_action, &error_code, &error_message, &error_status);
    if (result == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code != NULL ? error_code : "core_action.not_found", error_message != NULL ? error_message : "Expected core action was not found", error_status);
      return;
    }
  } else if (g_strcmp0(tool, "runtime.core_step") == 0) {
    JsonObject *args = object_object(body, "args");
    if (args == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.core_step requires args object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    const gchar *app_id = object_string(args, "appId", NULL);
    JsonNode *event = json_object_get_member(args, "event");
    if (app_id == NULL || app_id[0] == '\0' || event == NULL || !JSON_NODE_HOLDS_OBJECT(event)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "invalid_request", "runtime.core_step requires appId and event object", SOUP_STATUS_BAD_REQUEST);
      return;
    }
    g_autofree gchar *error_code = NULL;
    g_autofree gchar *error_message = NULL;
    guint error_status = SOUP_STATUS_BAD_REQUEST;
    if (plane->bridge == NULL) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, "platform_unsupported", "Linux dev control bridge is not available", SOUP_STATUS_SERVICE_UNAVAILABLE);
      return;
    }
    if (!control_session_allows_app(plane, control_session_id, app_id, &error_code, &error_message, &error_status)) {
      g_object_unref(parser);
      send_control_route_error(plane, message, control_session_id, tool, method, path, started, error_code, error_message, error_status);
      return;
    }
    const gchar *request_id = object_string(args, "id", "control_core_step");
    g_autofree gchar *bridge_body = core_step_request_json(request_id, event);
    AppSandboxContext context = app_sandbox_context_for_app(app_id, control_session_id);
    result = web_bridge_handle_json(plane->bridge, bridge_body, context);
  } else {
    g_object_unref(parser);
    send_control_route_error(plane, message, control_session_id, tool, method, path, started, "unsupported_tool", "Linux dev control session command is not supported yet", SOUP_STATUS_BAD_REQUEST);
    return;
  }
  g_object_unref(parser);

  if (result == NULL) {
    send_control_route_error(plane, message, control_session_id, tool, method, path, started, "not_found", error != NULL ? error->message : "Control session not found", SOUP_STATUS_BAD_REQUEST);
    g_clear_error(&error);
    return;
  }
  send_control_route_result(plane, message, control_session_id, tool, method, path, started, result);
}

static void control_route_handler(SoupServer *server, SoupServerMessage *message, const char *path, GHashTable *query, gpointer user_data) {
  (void)server;
  (void)query;
  DevControlPlane *plane = user_data;
  gint64 started = g_get_real_time();
  const gchar *method = soup_server_message_get_method(message);

  if (!authorize_request(plane, message, method, path, started)) {
    return;
  }

  if (is_sessions_collection_path(path)) {
    session_create_handler(plane, message, method, path, started);
    return;
  }

  g_autofree gchar *snapshot_session_id = session_id_from_path(path, "/snapshot");
  if (snapshot_session_id != NULL) {
    session_snapshot_handler(plane, message, method, path, started, snapshot_session_id);
    return;
  }

  g_autofree gchar *events_session_id = session_id_from_path(path, "/events");
  if (events_session_id != NULL) {
    session_events_handler(plane, message, method, path, started, events_session_id);
    return;
  }

  g_autofree gchar *capabilities_session_id = session_id_from_path(path, "/capabilities");
  if (capabilities_session_id != NULL) {
    session_capabilities_handler(plane, message, method, path, started, capabilities_session_id);
    return;
  }

  g_autofree gchar *command_session_id = session_id_from_path(path, "/command");
  if (command_session_id != NULL) {
    session_command_handler(plane, message, method, path, started, command_session_id);
    return;
  }

  g_autofree gchar *item_session_id = session_id_from_path(path, NULL);
  if (item_session_id != NULL) {
    session_item_handler(plane, message, method, path, started, item_session_id);
    return;
  }

  g_autofree gchar *body = error_json("not_found", "Control route was not found");
  send_json(message, SOUP_STATUS_NOT_FOUND, body);
  audit_control_request(plane, NULL, is_sessions_route_path(path) ? "control.sessions" : "control.route", method, path, "rejected", "not_found", NULL, body, (g_get_real_time() - started) / 1000);
}

static guint bound_port(SoupServer *server) {
  GSList *uris = soup_server_get_uris(server);
  guint port = 0;
  if (uris != NULL && uris->data != NULL) {
    port = (guint)g_uri_get_port((GUri *)uris->data);
  }
  g_slist_free_full(uris, (GDestroyNotify)g_uri_unref);
  return port;
}

DevControlPlane *dev_control_plane_start(const DevControlPlaneConfig *config, GError **error) {
  if (config == NULL || config->database_path == NULL || config->database_path[0] == '\0') {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_INVAL, "Linux dev control requires a database path");
    return NULL;
  }

  DevControlPlane *plane = g_new0(DevControlPlane, 1);
  plane->database_path = g_strdup(config->database_path);
  plane->control_session_id = g_strdup_printf("linux-control-session-%d-%" G_GINT64_FORMAT, getpid(), g_get_real_time());
  plane->token_path = token_file_path(error);
  plane->token = plane->token_path == NULL ? NULL : generate_control_token(error);
  if (plane->token_path == NULL || plane->token == NULL || !write_control_token_file(plane->token_path, plane->token, error)) {
    dev_control_plane_stop(plane);
    return NULL;
  }
  plane->token_hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, plane->token, -1);

  plane->server = soup_server_new("server-header", "NativeAIWebappLinuxDevControl", NULL);
  soup_server_add_handler(plane->server, "/health", health_handler, plane, NULL);
  soup_server_add_handler(plane->server, NULL, control_route_handler, plane, NULL);

  GInetAddress *address = g_inet_address_new_loopback(G_SOCKET_FAMILY_IPV4);
  GSocketAddress *socket_address = g_inet_socket_address_new(address, config->requested_port);
  gboolean listening = soup_server_listen(plane->server, socket_address, 0, error);
  g_object_unref(socket_address);
  g_object_unref(address);
  if (!listening) {
    dev_control_plane_stop(plane);
    return NULL;
  }

  plane->port = bound_port(plane->server);
  insert_control_session(plane);
  return plane;
}

void dev_control_plane_set_bridge(DevControlPlane *plane, WebBridge *bridge) {
  if (plane == NULL) {
    return;
  }
  plane->bridge = bridge;
  if (!plane->ready_announced && plane->bridge != NULL) {
    plane->ready_announced = TRUE;
    g_print("NATIVE_AI_LINUX_CONTROL_READY port=%u token_path=%s\n", plane->port, plane->token_path);
  }
}

void dev_control_plane_stop(DevControlPlane *plane) {
  if (plane == NULL) {
    return;
  }
  if (plane->server != NULL) {
    soup_server_disconnect(plane->server);
    g_clear_object(&plane->server);
  }
  if (plane->control_session_id != NULL && plane->database_path != NULL) {
    finish_control_session(plane);
  }
  g_free(plane->database_path);
  g_free(plane->control_session_id);
  g_free(plane->token);
  g_free(plane->token_hash);
  g_free(plane->token_path);
  g_free(plane);
}

guint dev_control_plane_port(const DevControlPlane *plane) {
  return plane == NULL ? 0 : plane->port;
}

const gchar *dev_control_plane_token_path(const DevControlPlane *plane) {
  return plane == NULL ? NULL : plane->token_path;
}
#else
struct _DevControlPlane {
  guint disabled;
};

DevControlPlane *dev_control_plane_start(const DevControlPlaneConfig *config, GError **error) {
  (void)config;
  g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_INVAL, "Linux dev control plane is disabled in release builds");
  return NULL;
}

void dev_control_plane_set_bridge(DevControlPlane *plane, WebBridge *bridge) {
  (void)plane;
  (void)bridge;
}

void dev_control_plane_stop(DevControlPlane *plane) {
  g_free(plane);
}

guint dev_control_plane_port(const DevControlPlane *plane) {
  (void)plane;
  return 0;
}

const gchar *dev_control_plane_token_path(const DevControlPlane *plane) {
  (void)plane;
  return NULL;
}
#endif
