#include "platform_storage.h"

static JsonNode *storage_error(const BridgeRequest *request, sqlite3 *db, const gchar *operation) {
  JsonObject *details = json_object_new();
  json_object_set_string_member(details, "sqliteMessage", db != NULL ? sqlite3_errmsg(db) : "database unavailable");
  g_autofree gchar *message = g_strdup_printf("%s failed", operation);
  return bridge_failure(request, "storage_error", message, details);
}

static gboolean ensure_app_row(PlatformStorage *storage, const gchar *app_id) {
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
      storage->db,
      "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, datetime('now'), datetime('now'))",
      -1,
      &statement,
      NULL) != SQLITE_OK) {
    return FALSE;
  }
  sqlite3_bind_text(statement, 1, app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, app_id, -1, SQLITE_TRANSIENT);
  gboolean ok = sqlite3_step(statement) == SQLITE_DONE;
  sqlite3_finalize(statement);
  return ok;
}

static gboolean has_storage_prefix(const BridgeRequest *request, const gchar *key) {
  return g_str_has_prefix(key, request->context.storage_prefix);
}

static gboolean resource_budget_limit(const BridgeRequest *request, const gchar *name, guint *out) {
  if (request == NULL || request->context.resource_budget == NULL) {
    return FALSE;
  }
  gpointer value = NULL;
  if (!g_hash_table_lookup_extended(request->context.resource_budget, name, NULL, &value)) {
    return FALSE;
  }
  *out = GPOINTER_TO_UINT(value);
  return TRUE;
}

static gboolean storage_bytes_after_set(PlatformStorage *storage, const gchar *app_id, const gchar *key, gint64 value_bytes, gint64 *out) {
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
      storage->db,
      "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ? AND key != ?",
      -1,
      &statement,
      NULL) != SQLITE_OK) {
    return FALSE;
  }
  sqlite3_bind_text(statement, 1, app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);
  if (sqlite3_step(statement) != SQLITE_ROW) {
    sqlite3_finalize(statement);
    return FALSE;
  }
  gint64 current_other_bytes = sqlite3_column_int64(statement, 0);
  sqlite3_finalize(statement);
  *out = current_other_bytes + value_bytes;
  return TRUE;
}

static JsonNode *storage_prefix_failure(const BridgeRequest *request, const gchar *key) {
  JsonObject *details = json_object_new();
  json_object_set_string_member(details, "key", key);
  json_object_set_string_member(details, "prefix", request->context.storage_prefix);
  json_object_set_string_member(details, "appId", request->context.app_id);
  return bridge_failure(request, "permission_denied", "Storage key must begin with app storage prefix", details);
}

PlatformStorage *platform_storage_new(const gchar *database_path) {
  PlatformStorage *storage = g_new0(PlatformStorage, 1);
  storage->db = platform_database_open(database_path);
  return storage;
}

void platform_storage_free(PlatformStorage *storage) {
  if (storage == NULL) {
    return;
  }
  platform_database_close(storage->db);
  g_free(storage);
}

JsonNode *platform_storage_get(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.get requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(storage->db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, NULL) != SQLITE_OK) {
    return storage_error(request, storage->db, "storage.get");
  }
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "value");
  gint step = sqlite3_step(statement);
  if (step == SQLITE_ROW) {
    const gchar *json_text = (const gchar *)sqlite3_column_text(statement, 0);
    JsonParser *parser = json_parser_new();
    if (json_text != NULL && json_parser_load_from_data(parser, json_text, -1, NULL)) {
      json_builder_add_value(builder, json_node_copy(json_parser_get_root(parser)));
    } else {
      json_builder_add_null_value(builder);
    }
    g_object_unref(parser);
  } else if (step == SQLITE_DONE) {
    json_builder_add_null_value(builder);
  } else {
    sqlite3_finalize(statement);
    g_object_unref(builder);
    return storage_error(request, storage->db, "storage.get");
  }
  json_builder_end_object(builder);
  sqlite3_finalize(statement);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_set(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.set requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }
  if (!ensure_app_row(storage, request->context.app_id)) {
    return storage_error(request, storage->db, "storage.set");
  }

  JsonGenerator *generator = json_generator_new();
  JsonNode *null_value = NULL;
  JsonNode *value = json_object_get_member(request->params, "value");
  if (value == NULL) {
    null_value = json_node_new(JSON_NODE_NULL);
    value = null_value;
  }
  json_generator_set_root(generator, value);
  g_autofree gchar *value_json = json_generator_to_data(generator, NULL);
  if (null_value != NULL) {
    json_node_unref(null_value);
  }
  if (value_json == NULL) {
    g_object_unref(generator);
    return bridge_failure(request, "invalid_request", "storage.set value must be JSON-serializable", NULL);
  }
  guint limit = 0;
  if (resource_budget_limit(request, "maxStorageBytes", &limit)) {
    gint64 projected_bytes = 0;
    if (!storage_bytes_after_set(storage, request->context.app_id, key, (gint64)strlen(value_json), &projected_bytes)) {
      g_object_unref(generator);
      return storage_error(request, storage->db, "storage.set");
    }
    if (projected_bytes > (gint64)limit) {
      JsonObject *details = json_object_new();
      json_object_set_string_member(details, "appId", request->context.app_id);
      json_object_set_string_member(details, "key", key);
      json_object_set_string_member(details, "budget", "maxStorageBytes");
      json_object_set_int_member(details, "current", projected_bytes);
      json_object_set_int_member(details, "max", limit);
      json_object_set_int_member(details, "limit", limit);
      json_object_set_int_member(details, "projectedBytes", projected_bytes);
      g_object_unref(generator);
      return bridge_failure(request, "resource_budget_exceeded", "Storage write exceeds manifest.resourceBudget.maxStorageBytes", details);
    }
  }
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(
      storage->db,
      "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now')) "
      "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
      -1,
      &statement,
      NULL) != SQLITE_OK) {
    g_object_unref(generator);
    return storage_error(request, storage->db, "storage.set");
  }
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 3, value_json, -1, SQLITE_TRANSIENT);
  if (sqlite3_step(statement) != SQLITE_DONE) {
    sqlite3_finalize(statement);
    g_object_unref(generator);
    return storage_error(request, storage->db, "storage.set");
  }
  sqlite3_finalize(statement);
  g_object_unref(generator);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "bytesWritten");
  json_builder_add_int_value(builder, strlen(value_json));
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_remove(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.remove requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(storage->db, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, NULL) != SQLITE_OK) {
    return storage_error(request, storage->db, "storage.remove");
  }
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);
  if (sqlite3_step(statement) != SQLITE_DONE) {
    sqlite3_finalize(statement);
    return storage_error(request, storage->db, "storage.remove");
  }
  sqlite3_finalize(statement);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_list(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *prefix = json_object_get_string_member_with_default(request->params, "prefix", "");
  if (*prefix == '\0') {
    return bridge_failure(request, "invalid_request", "storage.list requires prefix", NULL);
  }
  if (!has_storage_prefix(request, prefix)) {
    return storage_prefix_failure(request, prefix);
  }

  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(storage->db, "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", -1, &statement, NULL) != SQLITE_OK) {
    return storage_error(request, storage->db, "storage.list");
  }
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  g_autofree gchar *like_prefix = g_strdup_printf("%s%%", prefix);
  sqlite3_bind_text(statement, 2, like_prefix, -1, SQLITE_TRANSIENT);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "keys");
  json_builder_begin_array(builder);
  gint step = SQLITE_ROW;
  while ((step = sqlite3_step(statement)) == SQLITE_ROW) {
    json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
  }
  if (step != SQLITE_DONE) {
    sqlite3_finalize(statement);
    g_object_unref(builder);
    return storage_error(request, storage->db, "storage.list");
  }
  json_builder_end_array(builder);
  json_builder_end_object(builder);
  sqlite3_finalize(statement);
  return bridge_success(request, json_builder_get_root(builder));
}
