#include "dev_control_plane.h"

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
#include <libsoup/soup.h>

struct _DevControlPlane {
  SoupServer *server;
  gchar *database_path;
  gchar *control_session_id;
  gchar *token;
  gchar *token_hash;
  gchar *token_path;
  guint port;
};

static void bind_text(sqlite3_stmt *statement, int index, const gchar *value) {
  sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT);
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
  g_autofree gchar *metadata = g_strdup_printf("{\"port\":%u,\"tokenPath\":\"%s\"}", plane->port, escaped_token_path);
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT OR REPLACE INTO control_sessions "
          "(control_session_id, target, actor, token_hash, started_at, status, metadata_json) "
          "VALUES (?, 'linux', 'codex', ?, datetime('now'), 'running', ?)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, plane->control_session_id);
    bind_text(statement, 2, plane->token_hash);
    bind_text(statement, 3, metadata);
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
          "UPDATE control_sessions SET ended_at = datetime('now'), status = 'stopped' WHERE control_session_id = ?",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, plane->control_session_id);
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
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
          db,
          "INSERT INTO control_commands "
          "(command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) "
          "VALUES (?, ?, 'platform.health', ?, ?, ?, ?, '{}', ?, ?, datetime('now'), ?)",
          -1,
          &statement,
          NULL) == SQLITE_OK) {
    bind_text(statement, 1, command_id);
    bind_text(statement, 2, plane->control_session_id);
    bind_text(statement, 3, method);
    bind_text(statement, 4, path);
    bind_text(statement, 5, decision);
    if (error_code == NULL) {
      sqlite3_bind_null(statement, 6);
    } else {
      bind_text(statement, 6, error_code);
    }
    if (result_json == NULL) {
      sqlite3_bind_null(statement, 7);
    } else {
      bind_text(statement, 7, result_json);
    }
    if (error_body == NULL) {
      sqlite3_bind_null(statement, 8);
    } else {
      bind_text(statement, 8, error_body);
    }
    sqlite3_bind_int64(statement, 9, duration_ms);
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
  audit_control_request(plane, method, path, "rejected", "control_auth_required", NULL, body, (g_get_real_time() - started) / 1000);
  return FALSE;
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
    audit_control_request(plane, method, path, "rejected", "method_not_allowed", NULL, body, (g_get_real_time() - started) / 1000);
    return;
  }

  g_autofree gchar *body = health_result_json(plane);
  send_json(message, SOUP_STATUS_OK, body);
  audit_control_request(plane, method, path, "accepted", NULL, body, NULL, (g_get_real_time() - started) / 1000);
}

static void not_found_handler(SoupServer *server, SoupServerMessage *message, const char *path, GHashTable *query, gpointer user_data) {
  (void)server;
  (void)query;
  DevControlPlane *plane = user_data;
  gint64 started = g_get_real_time();
  const gchar *method = soup_server_message_get_method(message);
  if (!authorize_request(plane, message, method, path, started)) {
    return;
  }
  g_autofree gchar *body = error_json("not_found", "Control route was not found");
  send_json(message, SOUP_STATUS_NOT_FOUND, body);
  audit_control_request(plane, method, path, "rejected", "not_found", NULL, body, (g_get_real_time() - started) / 1000);
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
  soup_server_add_handler(plane->server, NULL, not_found_handler, plane, NULL);

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
  g_print("NATIVE_AI_LINUX_CONTROL_READY port=%u token_path=%s\n", plane->port, plane->token_path);
  return plane;
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
