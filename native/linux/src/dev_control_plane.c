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

static gchar *now_iso(void) {
  GDateTime *now = g_date_time_new_now_utc();
  gchar *text = g_date_time_format_iso8601(now);
  g_date_time_unref(now);
  return text;
}

static gchar *make_id(const gchar *prefix) {
  return g_strdup_printf("%s-%d-%" G_GINT64_FORMAT "-%08x", prefix, getpid(), g_get_real_time(), g_random_int());
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

static gchar *json_builder_to_text(JsonBuilder *builder) {
  JsonNode *root = json_builder_get_root(builder);
  gchar *text = json_node_to_text(root);
  json_node_unref(root);
  return text;
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

static void append_bridge_call_rows(JsonBuilder *builder, sqlite3 *db, const gchar *app_id) {
  const gchar *sql = app_id == NULL
      ? "SELECT bridge_call_id, session_id, app_id, method, created_at FROM bridge_calls ORDER BY created_at"
      : "SELECT bridge_call_id, session_id, app_id, method, created_at FROM bridge_calls WHERE app_id = ? ORDER BY created_at";
  sqlite3_stmt *statement = NULL;
  json_builder_begin_array(builder);
  if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) == SQLITE_OK) {
    if (app_id != NULL) {
      bind_text(statement, 1, app_id);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      json_builder_begin_object(builder);
      json_builder_set_member_name(builder, "bridgeCallId");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
      json_builder_set_member_name(builder, "sessionId");
      sqlite3_column_text(statement, 1) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 1));
      json_builder_set_member_name(builder, "appId");
      sqlite3_column_text(statement, 2) == NULL ? json_builder_add_null_value(builder) : json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 2));
      json_builder_set_member_name(builder, "method");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 3));
      json_builder_set_member_name(builder, "createdAt");
      json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 4));
      json_builder_end_object(builder);
    }
  }
  sqlite3_finalize(statement);
  json_builder_end_array(builder);
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

typedef struct {
  const gchar *table;
  const gchar * const *columns;
  gsize column_count;
  const gchar *order_by;
  const gchar *filter_column;
} SafeDbTable;

static const gchar * const db_apps_columns[] = {"id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"};
static const gchar * const db_app_versions_columns[] = {"install_id", "app_id", "version", "runtime_version", "data_version", "content_hash", "status", "created_at", "activated_at"};
static const gchar * const db_app_storage_columns[] = {"app_id", "key", "value_json", "updated_at"};
static const gchar * const db_bridge_calls_columns[] = {"bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"};
static const gchar * const db_core_events_columns[] = {"event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"};
static const gchar * const db_test_runs_columns[] = {"test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"};
static const gchar * const db_control_sessions_columns[] = {"control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"};
static const gchar * const db_control_commands_columns[] = {"command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "created_at", "duration_ms"};
static const gchar * const db_runtime_sessions_columns[] = {"session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"};
static const gchar * const db_runtime_snapshots_columns[] = {"snapshot_id", "session_id", "app_id", "install_id", "type", "content_hash", "created_at"};
static const gchar * const db_backup_exports_columns[] = {"export_id", "type", "source_platform", "runtime_version", "content_hash", "created_at", "imported_at"};

static const SafeDbTable safe_db_apps = {"apps", db_apps_columns, G_N_ELEMENTS(db_apps_columns), "id", NULL};
static const SafeDbTable safe_db_app_versions = {"app_versions", db_app_versions_columns, G_N_ELEMENTS(db_app_versions_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_app_storage = {"app_storage", db_app_storage_columns, G_N_ELEMENTS(db_app_storage_columns), "updated_at", "app_id"};
static const SafeDbTable safe_db_bridge_calls = {"bridge_calls", db_bridge_calls_columns, G_N_ELEMENTS(db_bridge_calls_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_core_events = {"core_events", db_core_events_columns, G_N_ELEMENTS(db_core_events_columns), "created_at", "app_id"};
static const SafeDbTable safe_db_test_runs = {"test_runs", db_test_runs_columns, G_N_ELEMENTS(db_test_runs_columns), "started_at", "app_id"};
static const SafeDbTable safe_db_control_sessions = {"control_sessions", db_control_sessions_columns, G_N_ELEMENTS(db_control_sessions_columns), "started_at", NULL};
static const SafeDbTable safe_db_control_commands = {"control_commands", db_control_commands_columns, G_N_ELEMENTS(db_control_commands_columns), "created_at", NULL};
static const SafeDbTable safe_db_runtime_sessions = {"runtime_sessions", db_runtime_sessions_columns, G_N_ELEMENTS(db_runtime_sessions_columns), "started_at", NULL};
static const SafeDbTable safe_db_runtime_snapshots = {"runtime_snapshots", db_runtime_snapshots_columns, G_N_ELEMENTS(db_runtime_snapshots_columns), "created_at", NULL};
static const SafeDbTable safe_db_backup_exports = {"backup_exports", db_backup_exports_columns, G_N_ELEMENTS(db_backup_exports_columns), "created_at", NULL};

static const SafeDbTable * const db_snapshot_tables[] = {
    &safe_db_apps,
    &safe_db_app_versions,
    &safe_db_app_storage,
    &safe_db_bridge_calls,
    &safe_db_core_events,
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
  } else if (g_strcmp0(tool, "runtime.capabilities") == 0) {
    result = session_capabilities_json(plane, control_session_id, &error);
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
